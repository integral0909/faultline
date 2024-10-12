//! RAML type expressions: `Pet`, `Pet[]`, `string | nil`, `(Cat | Dog)[]`,
//! `string?` (shorthand for `string | nil`) and `lib.Pet` (a type from a
//! library used under the name `lib`).

use std::fmt;

/// Expressions nest at most this deep.
const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeExpr {
    /// A built-in or declared type, possibly qualified by a library name.
    Name(String),
    Array(Box<TypeExpr>),
    Union(Vec<TypeExpr>),
    /// `T?`: `T` or `nil`.
    Nullable(Box<TypeExpr>),
}

impl fmt::Display for TypeExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeExpr::Name(name) => f.write_str(name),
            TypeExpr::Array(item) => match item.as_ref() {
                TypeExpr::Union(_) => write!(f, "({item})[]"),
                _ => write!(f, "{item}[]"),
            },
            TypeExpr::Union(members) => {
                let parts: Vec<String> = members.iter().map(ToString::to_string).collect();
                f.write_str(&parts.join(" | "))
            }
            TypeExpr::Nullable(inner) => write!(f, "{inner}?"),
        }
    }
}

/// Parses a type expression.
pub fn parse(text: &str) -> Result<TypeExpr, String> {
    let mut parser = Parser {
        chars: text.chars().collect(),
        pos: 0,
    };
    let expr = parser.union(0)?;
    parser.skip_space();
    if parser.pos < parser.chars.len() {
        return Err(format!(
            "unexpected {:?} in type expression {text:?}",
            parser.chars[parser.pos]
        ));
    }
    Ok(expr)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn skip_space(&mut self) {
        while self.chars.get(self.pos).is_some_and(|c| c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.skip_space();
        self.chars.get(self.pos).copied()
    }

    fn union(&mut self, depth: usize) -> Result<TypeExpr, String> {
        if depth > MAX_DEPTH {
            return Err("the type expression nests too deeply".into());
        }
        let mut members = vec![self.postfix(depth)?];
        while self.peek() == Some('|') {
            self.pos += 1;
            members.push(self.postfix(depth)?);
        }
        Ok(if members.len() == 1 {
            members.remove(0)
        } else {
            TypeExpr::Union(members)
        })
    }

    fn postfix(&mut self, depth: usize) -> Result<TypeExpr, String> {
        let mut expr = self.primary(depth)?;
        loop {
            match self.peek() {
                Some('[') => {
                    self.pos += 1;
                    if self.peek() != Some(']') {
                        return Err("`[` must be followed by `]`".into());
                    }
                    self.pos += 1;
                    expr = TypeExpr::Array(Box::new(expr));
                }
                Some('?') => {
                    self.pos += 1;
                    expr = TypeExpr::Nullable(Box::new(expr));
                }
                _ => return Ok(expr),
            }
        }
    }

    fn primary(&mut self, depth: usize) -> Result<TypeExpr, String> {
        match self.peek() {
            Some('(') => {
                self.pos += 1;
                let inner = self.union(depth + 1)?;
                if self.peek() != Some(')') {
                    return Err("`(` is not closed".into());
                }
                self.pos += 1;
                Ok(inner)
            }
            Some(c) if is_name_char(c) => {
                let start = self.pos;
                while self.chars.get(self.pos).copied().is_some_and(is_name_char) {
                    self.pos += 1;
                }
                Ok(TypeExpr::Name(self.chars[start..self.pos].iter().collect()))
            }
            Some(c) => Err(format!("expected a type name, found {c:?}")),
            None => Err("expected a type name".into()),
        }
    }
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(n: &str) -> TypeExpr {
        TypeExpr::Name(n.into())
    }

    #[test]
    fn expressions() {
        assert_eq!(parse("Pet").unwrap(), name("Pet"));
        assert_eq!(parse(" lib.Pet ").unwrap(), name("lib.Pet"));
        assert_eq!(
            parse("Pet[]").unwrap(),
            TypeExpr::Array(Box::new(name("Pet")))
        );
        assert_eq!(
            parse("string | nil").unwrap(),
            TypeExpr::Union(vec![name("string"), name("nil")])
        );
        assert_eq!(
            parse("(Cat | Dog)[][]").unwrap(),
            TypeExpr::Array(Box::new(TypeExpr::Array(Box::new(TypeExpr::Union(vec![
                name("Cat"),
                name("Dog")
            ])))))
        );
        assert_eq!(
            parse("string?").unwrap(),
            TypeExpr::Nullable(Box::new(name("string")))
        );
        assert_eq!(parse("A | B[]").unwrap().to_string(), "A | B[]");
        assert_eq!(parse("(A | B)[]").unwrap().to_string(), "(A | B)[]");
    }

    #[test]
    fn malformed_expressions() {
        for bad in ["", "Pet[", "(A | B", "A |", "| A", "A B", "A[x]", "{}"] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        let deep = format!("{}A{}", "(".repeat(100), ")".repeat(100));
        assert!(parse(&deep).is_err());
    }
}

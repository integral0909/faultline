//! Glob patterns over [`IrPath`](faultline_ir::IrPath) strings.
//!
//! `*` matches any run of characters except `/`, `**` any run including `/`,
//! and `?` one character other than `/`. Everything else matches itself, so
//! the brackets of path segments need no escaping:
//! `op[* /internal/**]` matches every operation under `/internal/`.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Literal(char),
    Any,
    AnyDeep,
    One,
}

/// A compiled glob.
#[derive(Clone, PartialEq, Eq)]
pub struct Glob {
    source: String,
    tokens: Vec<Token>,
}

impl Glob {
    pub fn new(pattern: &str) -> Glob {
        let mut tokens = Vec::new();
        let mut chars = pattern.chars().peekable();
        while let Some(c) = chars.next() {
            tokens.push(match c {
                '*' if chars.peek() == Some(&'*') => {
                    chars.next();
                    Token::AnyDeep
                }
                '*' => Token::Any,
                '?' => Token::One,
                c => Token::Literal(c),
            });
        }
        Glob {
            source: pattern.to_owned(),
            tokens,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Whether the whole of `text` matches.
    pub fn matches(&self, text: &str) -> bool {
        let text: Vec<char> = text.chars().collect();
        // Dynamic programming over (token, position); linear in both, so
        // patterns with many stars cannot cause exponential backtracking.
        let mut reachable = vec![false; text.len() + 1];
        reachable[0] = true;
        for token in &self.tokens {
            let mut next = vec![false; text.len() + 1];
            match token {
                Token::Literal(c) => {
                    for i in 0..text.len() {
                        if reachable[i] && text[i] == *c {
                            next[i + 1] = true;
                        }
                    }
                }
                Token::One => {
                    for i in 0..text.len() {
                        if reachable[i] && text[i] != '/' {
                            next[i + 1] = true;
                        }
                    }
                }
                Token::Any | Token::AnyDeep => {
                    let deep = *token == Token::AnyDeep;
                    let mut carrying = false;
                    for i in 0..=text.len() {
                        carrying =
                            reachable[i] || (carrying && i > 0 && (deep || text[i - 1] != '/'));
                        next[i] = carrying;
                    }
                }
            }
            reachable = next;
        }
        reachable[text.len()]
    }
}

impl fmt::Debug for Glob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Glob({})", self.source)
    }
}

#[cfg(test)]
mod tests {
    use super::Glob;

    #[test]
    fn stars_and_double_stars() {
        let internal = Glob::new("op[* /internal/**]**");
        assert!(internal.matches("op[GET /internal/stats]"));
        assert!(internal.matches("op[POST /internal/a/b]/request[application/json].name"));
        assert!(!internal.matches("op[GET /public/internal/x]"));

        let single = Glob::new("op[GET /pets/*]");
        assert!(single.matches("op[GET /pets/{}]"));
        assert!(!single.matches("op[GET /pets/{}/toys]"));

        assert!(Glob::new("**").matches(""));
        assert!(Glob::new("type[?ag]").matches("type[Tag]"));
        assert!(!Glob::new("type[?ag]").matches("type[/ag]"));
        assert!(Glob::new("schema[s].name").matches("schema[s].name"));
        assert!(!Glob::new("schema[s].name").matches("schema[s].names"));
    }

    #[test]
    fn many_stars_do_not_backtrack_exponentially() {
        let pattern = Glob::new(&"*a".repeat(30));
        let text = "a".repeat(29) + "b";
        assert!(!pattern.matches(&text));
    }
}

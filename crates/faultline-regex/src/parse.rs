//! Parsing ECMA-262 patterns into a syntax tree of regular constructs.
//!
//! The grammar is the specification's, with the Annex B leniencies browsers
//! and JSON Schema validators accept: a `{` that does not start a quantifier
//! and a stray `]` or `}` are literal characters. Constructs that do not
//! describe a regular language (backreferences, lookaround, word boundaries)
//! and Unicode property escapes are reported as unsupported rather than
//! approximated.

use crate::charset::{CharSet, MAX};

/// A parsed pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// Matches the empty string.
    Empty,
    /// One character from the set.
    Set(CharSet),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: u32,
        max: Option<u32>,
    },
    /// `^`: only at the start of the input.
    Start,
    /// `$`: only at the end of the input.
    End,
}

/// Why a pattern could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The pattern is not valid ECMA-262.
    Syntax { offset: usize, message: String },
    /// The pattern is valid but uses a construct outside regular languages,
    /// or is too large to analyse.
    Unsupported(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Syntax { offset, message } => {
                write!(f, "invalid regular expression at {offset}: {message}")
            }
            Error::Unsupported(message) => write!(f, "unsupported regular expression: {message}"),
        }
    }
}

impl std::error::Error for Error {}

/// Quantifier bounds above this are not expanded.
pub const MAX_REPEAT: u32 = 1000;

const MAX_NESTING: usize = 200;

pub fn parse(pattern: &str) -> Result<Node, Error> {
    let mut parser = Parser {
        chars: pattern.chars().collect(),
        pos: 0,
        depth: 0,
    };
    let node = parser.disjunction()?;
    if parser.pos < parser.chars.len() {
        // Only an unbalanced `)` stops a disjunction early.
        return Err(parser.syntax("unmatched `)`"));
    }
    Ok(node)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    depth: usize,
}

fn line_terminators() -> CharSet {
    CharSet::from_ranges(&[(0x0A, 0x0A), (0x0D, 0x0D), (0x2028, 0x2029)])
}

fn digits() -> CharSet {
    CharSet::range(u32::from(b'0'), u32::from(b'9'))
}

fn word() -> CharSet {
    CharSet::from_ranges(&[
        (u32::from(b'0'), u32::from(b'9')),
        (u32::from(b'A'), u32::from(b'Z')),
        (u32::from(b'_'), u32::from(b'_')),
        (u32::from(b'a'), u32::from(b'z')),
    ])
}

fn space() -> CharSet {
    CharSet::from_ranges(&[
        (0x09, 0x0D),
        (0x20, 0x20),
        (0xA0, 0xA0),
        (0x1680, 0x1680),
        (0x2000, 0x200A),
        (0x2028, 0x2029),
        (0x202F, 0x202F),
        (0x205F, 0x205F),
        (0x3000, 0x3000),
        (0xFEFF, 0xFEFF),
    ])
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        let at = self.peek() == Some(c);
        if at {
            self.pos += 1;
        }
        at
    }

    fn syntax(&self, message: &str) -> Error {
        Error::Syntax {
            offset: self.pos,
            message: message.to_owned(),
        }
    }

    fn disjunction(&mut self) -> Result<Node, Error> {
        self.depth += 1;
        if self.depth > MAX_NESTING {
            return Err(Error::Unsupported("groups are nested too deeply".into()));
        }
        let mut alternatives = vec![self.alternative()?];
        while self.eat('|') {
            alternatives.push(self.alternative()?);
        }
        self.depth -= 1;
        Ok(if alternatives.len() == 1 {
            alternatives.pop().expect("one alternative")
        } else {
            Node::Alt(alternatives)
        })
    }

    fn alternative(&mut self) -> Result<Node, Error> {
        let mut terms = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            terms.push(self.term()?);
        }
        Ok(match terms.len() {
            0 => Node::Empty,
            1 => terms.pop().expect("one term"),
            _ => Node::Concat(terms),
        })
    }

    fn term(&mut self) -> Result<Node, Error> {
        let atom = match self.peek().expect("not at end") {
            '^' => {
                self.pos += 1;
                return Ok(Node::Start);
            }
            '$' => {
                self.pos += 1;
                return Ok(Node::End);
            }
            '(' => self.group()?,
            '.' => {
                self.pos += 1;
                Node::Set(line_terminators().complement())
            }
            '[' => Node::Set(self.class()?),
            '\\' => self.atom_escape()?,
            '*' | '+' | '?' => return Err(self.syntax("nothing to repeat")),
            '{' if self.quantifier_ahead() => return Err(self.syntax("nothing to repeat")),
            c => {
                self.pos += 1;
                Node::Set(CharSet::single(c as u32))
            }
        };
        let Some((min, max)) = self.quantifier() else {
            return Ok(atom);
        };
        if max.is_some_and(|max| max < min) {
            return Err(self.syntax("numbers out of order in quantifier"));
        }
        if min > MAX_REPEAT || max.is_some_and(|m| m > MAX_REPEAT) {
            return Err(Error::Unsupported(format!(
                "quantifier bounds above {MAX_REPEAT}"
            )));
        }
        Ok(Node::Repeat {
            node: Box::new(atom),
            min,
            max,
        })
    }

    /// Whether a `{` at the cursor starts a valid `{n}`, `{n,}` or `{n,m}`.
    fn quantifier_ahead(&self) -> bool {
        let mut i = self.pos + 1;
        let digits = |i: &mut usize| {
            let start = *i;
            while self.chars.get(*i).is_some_and(char::is_ascii_digit) {
                *i += 1;
            }
            *i > start
        };
        if !digits(&mut i) {
            return false;
        }
        if self.chars.get(i) == Some(&',') {
            i += 1;
            digits(&mut i);
        }
        self.chars.get(i) == Some(&'}')
    }

    fn number(&mut self) -> u32 {
        let mut value: u32 = 0;
        while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
            value = value.saturating_mul(10).saturating_add(d);
            self.pos += 1;
        }
        value
    }

    /// A quantifier and its lazy marker (which does not change the language).
    fn quantifier(&mut self) -> Option<(u32, Option<u32>)> {
        let bounds = match self.peek() {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            Some('{') if self.quantifier_ahead() => {
                self.pos += 1;
                let min = self.number();
                let max = if self.eat(',') {
                    if self.peek() == Some('}') {
                        None
                    } else {
                        Some(self.number())
                    }
                } else {
                    Some(min)
                };
                // quantifier_ahead guarantees the closing brace.
                self.pos += 1;
                self.eat('?');
                return Some((min, max));
            }
            _ => return None,
        };
        self.pos += 1;
        self.eat('?');
        Some(bounds)
    }

    fn group(&mut self) -> Result<Node, Error> {
        self.pos += 1;
        if self.eat('?') {
            match self.peek() {
                Some(':') => self.pos += 1,
                Some('=' | '!') => return Err(Error::Unsupported("lookahead".into())),
                Some('<') if matches!(self.peek_at(1), Some('=' | '!')) => {
                    return Err(Error::Unsupported("lookbehind".into()));
                }
                Some('<') => {
                    self.pos += 1;
                    while self.peek().is_some_and(|c| c != '>') {
                        self.pos += 1;
                    }
                    if !self.eat('>') {
                        return Err(self.syntax("unterminated group name"));
                    }
                }
                _ => return Err(self.syntax("invalid group")),
            }
        }
        let inner = self.disjunction()?;
        if !self.eat(')') {
            return Err(self.syntax("unterminated group"));
        }
        Ok(inner)
    }

    fn atom_escape(&mut self) -> Result<Node, Error> {
        self.pos += 1;
        let Some(c) = self.peek() else {
            return Err(self.syntax("`\\` at the end of the pattern"));
        };
        match c {
            'b' | 'B' => Err(Error::Unsupported("word boundaries".into())),
            '1'..='9' => Err(Error::Unsupported("backreferences".into())),
            'k' if self.peek_at(1) == Some('<') => Err(Error::Unsupported("backreferences".into())),
            _ => Ok(Node::Set(self.class_escape(false)?)),
        }
    }

    /// An escape usable in and out of classes; the cursor is after `\`.
    fn class_escape(&mut self, in_class: bool) -> Result<CharSet, Error> {
        let c = self.peek().expect("checked by the caller");
        self.pos += 1;
        let single = |v: u32| Ok(CharSet::single(v));
        match c {
            'd' => Ok(digits()),
            'D' => Ok(digits().complement()),
            'w' => Ok(word()),
            'W' => Ok(word().complement()),
            's' => Ok(space()),
            'S' => Ok(space().complement()),
            'b' if in_class => single(0x08),
            't' => single(0x09),
            'n' => single(0x0A),
            'v' => single(0x0B),
            'f' => single(0x0C),
            'r' => single(0x0D),
            '0' if !self.peek().is_some_and(|c| c.is_ascii_digit()) => single(0),
            'c' => match self.peek() {
                Some(letter) if letter.is_ascii_alphabetic() => {
                    self.pos += 1;
                    single(letter as u32 % 32)
                }
                // Annex B: `\c` not followed by a letter is a backslash
                // and a `c`.
                _ => {
                    self.pos -= 1;
                    single(u32::from(b'\\'))
                }
            },
            'x' => {
                let value = self.hex(2);
                match value {
                    Some(v) => single(v),
                    None => single(u32::from(b'x')),
                }
            }
            'u' => self.unicode_escape().map(CharSet::single),
            'p' | 'P' if self.peek() == Some('{') => {
                Err(Error::Unsupported("Unicode property escapes".into()))
            }
            // Identity escapes: `\.`, `\-`, `\/` and, under Annex B, any
            // character that has no other meaning.
            other => single(other as u32),
        }
    }

    fn hex(&mut self, digits: usize) -> Option<u32> {
        let text: String = self
            .chars
            .get(self.pos..self.pos + digits)?
            .iter()
            .collect();
        if text.len() != digits || !text.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        self.pos += digits;
        u32::from_str_radix(&text, 16).ok()
    }

    /// `\uXXXX`, a surrogate pair of them, or `\u{...}`; the cursor is
    /// after `u`.
    fn unicode_escape(&mut self) -> Result<u32, Error> {
        if self.eat('{') {
            let start = self.pos;
            while self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                self.pos += 1;
            }
            let text: String = self.chars[start..self.pos].iter().collect();
            if !self.eat('}') || text.is_empty() {
                return Err(self.syntax("invalid `\\u{...}` escape"));
            }
            return u32::from_str_radix(&text, 16)
                .ok()
                .filter(|v| *v <= MAX)
                .ok_or_else(|| self.syntax("code point out of range"));
        }
        let Some(high) = self.hex(4) else {
            // Annex B: `\u` without four digits is a literal `u`.
            return Ok(u32::from(b'u'));
        };
        if (0xD800..0xDC00).contains(&high)
            && self.peek() == Some('\\')
            && self.peek_at(1) == Some('u')
        {
            let save = self.pos;
            self.pos += 2;
            match self.hex(4) {
                Some(low) if (0xDC00..0xE000).contains(&low) => {
                    return Ok(0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00));
                }
                _ => self.pos = save,
            }
        }
        Ok(high)
    }

    /// `[...]` or `[^...]`.
    fn class(&mut self) -> Result<CharSet, Error> {
        self.pos += 1;
        let negated = self.eat('^');
        let mut set = CharSet::empty();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.syntax("unterminated character class"));
            };
            if c == ']' {
                self.pos += 1;
                break;
            }
            let (lo, lo_single) = self.class_atom()?;
            if self.peek() == Some('-') && self.peek_at(1).is_some_and(|c| c != ']') {
                self.pos += 1;
                let (hi, hi_single) = self.class_atom()?;
                match (lo_single, hi_single) {
                    (Some(a), Some(b)) if a <= b => set.add(a, b),
                    (Some(_), Some(_)) => {
                        return Err(self.syntax("range out of order in character class"));
                    }
                    // Annex B: a class escape in a range makes the `-`
                    // literal.
                    _ => {
                        set = set
                            .union(&lo)
                            .union(&CharSet::single(u32::from(b'-')))
                            .union(&hi);
                    }
                }
            } else {
                set = set.union(&lo);
            }
        }
        Ok(if negated { set.complement() } else { set })
    }

    /// One class member: the set, and the code point if it is a single one.
    fn class_atom(&mut self) -> Result<(CharSet, Option<u32>), Error> {
        let c = self.peek().expect("checked by the caller");
        self.pos += 1;
        if c != '\\' {
            return Ok((CharSet::single(c as u32), Some(c as u32)));
        }
        if self.peek().is_none() {
            return Err(self.syntax("`\\` at the end of the pattern"));
        }
        let class_escape = matches!(self.peek(), Some('d' | 'D' | 'w' | 'W' | 's' | 'S'));
        let set = self.class_escape(true)?;
        let single = if class_escape {
            None
        } else {
            set.ranges().first().map(|&(lo, _)| lo)
        };
        Ok((set, single))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_of(node: &Node) -> &CharSet {
        match node {
            Node::Set(s) => s,
            other => panic!("expected a set, got {other:?}"),
        }
    }

    #[test]
    fn literals_classes_and_quantifiers() {
        let node = parse("a[b-d]{2,3}?").unwrap();
        let Node::Concat(parts) = node else { panic!() };
        assert!(set_of(&parts[0]).contains(u32::from(b'a')));
        let Node::Repeat { node, min, max } = &parts[1] else {
            panic!()
        };
        assert_eq!((*min, *max), (2, Some(3)));
        assert_eq!(set_of(node).ranges(), [(u32::from(b'b'), u32::from(b'd'))]);
    }

    #[test]
    fn escapes() {
        assert!(set_of(&parse(r"\d").unwrap()).contains(u32::from(b'7')));
        assert!(!set_of(&parse(r"\D").unwrap()).contains(u32::from(b'7')));
        assert!(set_of(&parse(r"é").unwrap()).contains(0xE9));
        assert!(set_of(&parse(r"\u{1F600}").unwrap()).contains(0x1F600));
        assert!(set_of(&parse(r"😀").unwrap()).contains(0x1F600));
        assert!(set_of(&parse(r"\x41").unwrap()).contains(0x41));
        assert!(set_of(&parse(r"\.").unwrap()).contains(u32::from(b'.')));
        assert!(set_of(&parse(r"[\b]").unwrap()).contains(0x08));
        assert!(set_of(&parse(r"[\w-]").unwrap()).contains(u32::from(b'-')));
        assert!(set_of(&parse(r"[^a]").unwrap()).contains(u32::from(b'b')));
    }

    #[test]
    fn annex_b_leniencies() {
        assert!(set_of(&parse("{").unwrap()).contains(u32::from(b'{')));
        assert!(matches!(parse("a{,5}").unwrap(), Node::Concat(_)));
        assert!(set_of(&parse("]").unwrap()).contains(u32::from(b']')));
        // `\u` without four hex digits is a literal `u`.
        let Node::Concat(parts) = parse(r"\u12").unwrap() else {
            panic!()
        };
        assert!(set_of(&parts[0]).contains(u32::from(b'u')));
    }

    #[test]
    fn unsupported_and_invalid() {
        for pattern in [r"(a)\1", "(?=a)", "(?<!a)", r"\bword", r"\p{L}", "a{1001}"] {
            assert!(
                matches!(parse(pattern), Err(Error::Unsupported(_))),
                "{pattern}"
            );
        }
        for pattern in ["(", "a)", "[a", "*", "a{3,2}", "[z-a]", "\\"] {
            assert!(
                matches!(parse(pattern), Err(Error::Syntax { .. })),
                "{pattern}"
            );
        }
        assert!(parse(&"(".repeat(300)).is_err());
    }
}

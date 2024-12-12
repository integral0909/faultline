//! Parsing JSONPath queries as RFC 9535 defines them, including the
//! well-typedness rules for function expressions.

use std::fmt;

use faultline_doc::Decimal;

use crate::ast::{
    Argument, Comparable, CompareOp, Function, FunctionName, Literal, Logical, Query, Segment,
    Selector, Type,
};

/// Expressions nest at most this deep.
const MAX_DEPTH: usize = 64;

/// The largest integer JSONPath allows: 2^53 - 1, the largest integer every
/// JSON implementation represents exactly.
pub const MAX_INT: i64 = (1 << 53) - 1;

/// Why a query does not parse, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    /// Byte offset in the query.
    pub offset: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at offset {})", self.message, self.offset)
    }
}

impl std::error::Error for ParseError {}

type Parsed<T> = Result<T, ParseError>;

/// Parses a query, which starts with `$`.
pub fn parse(text: &str) -> Parsed<Query> {
    let mut parser = Parser { text, pos: 0 };
    if !text.starts_with('$') {
        return parser.fail("a query starts with `$`");
    }
    let query = parser.query(0)?;
    if parser.pos < text.len() {
        return parser.fail("unexpected text after the query");
    }
    Ok(query)
}

struct Parser<'t> {
    text: &'t str,
    pos: usize,
}

impl Parser<'_> {
    fn fail<T>(&self, message: &str) -> Parsed<T> {
        Err(ParseError {
            message: message.to_owned(),
            offset: self.pos,
        })
    }

    fn rest(&self) -> &str {
        &self.text[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn eat(&mut self, token: &str) -> bool {
        if self.rest().starts_with(token) {
            self.pos += token.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &str) -> Parsed<()> {
        if self.eat(token) {
            Ok(())
        } else {
            self.fail(&format!("expected `{token}`"))
        }
    }

    /// Blank space: spaces, tabs and line breaks.
    fn blank(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.pos += 1;
        }
    }

    /// `$` or `@`, then segments.
    fn query(&mut self, depth: usize) -> Parsed<Query> {
        let relative = match self.peek() {
            Some('$') => false,
            Some('@') => true,
            _ => return self.fail("expected `$` or `@`"),
        };
        self.pos += 1;
        let mut segments = Vec::new();
        loop {
            let before = self.pos;
            self.blank();
            let descendant = self.eat("..");
            if descendant {
                segments.push(self.descendant(depth)?);
            } else if self.eat(".") {
                segments.push(Segment {
                    descendant: false,
                    selectors: vec![self.shorthand()?],
                });
            } else if self.peek() == Some('[') {
                segments.push(Segment {
                    descendant: false,
                    selectors: self.bracketed(depth)?,
                });
            } else {
                // Blank space belongs to what follows the query.
                self.pos = before;
                return Ok(Query { relative, segments });
            }
        }
    }

    fn descendant(&mut self, depth: usize) -> Parsed<Segment> {
        let selectors = if self.peek() == Some('[') {
            self.bracketed(depth)?
        } else {
            vec![self.shorthand()?]
        };
        Ok(Segment {
            descendant: true,
            selectors,
        })
    }

    /// `*` or a member name, after `.` or `..`.
    fn shorthand(&mut self) -> Parsed<Selector> {
        if self.eat("*") {
            return Ok(Selector::Wildcard);
        }
        let start = self.pos;
        let first = |c: char| c.is_ascii_alphabetic() || c == '_' || !c.is_ascii();
        match self.peek() {
            Some(c) if first(c) => {}
            _ => return self.fail("expected a member name or `*`"),
        }
        while let Some(c) = self.peek() {
            if first(c) || c.is_ascii_digit() {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        Ok(Selector::Name(self.text[start..self.pos].to_owned()))
    }

    /// `[` selectors separated by `,` `]`.
    fn bracketed(&mut self, depth: usize) -> Parsed<Vec<Selector>> {
        self.expect("[")?;
        let mut selectors = Vec::new();
        loop {
            self.blank();
            selectors.push(self.selector(depth)?);
            self.blank();
            if self.eat("]") {
                return Ok(selectors);
            }
            self.expect(",")?;
        }
    }

    fn selector(&mut self, depth: usize) -> Parsed<Selector> {
        match self.peek() {
            Some('\'' | '"') => Ok(Selector::Name(self.string()?)),
            Some('*') => {
                self.pos += 1;
                Ok(Selector::Wildcard)
            }
            Some('?') => {
                self.pos += 1;
                self.blank();
                Ok(Selector::Filter(self.logical(depth + 1)?))
            }
            Some(':') => self.slice(None),
            Some(c) if c == '-' || c.is_ascii_digit() => {
                let n = self.integer()?;
                let before = self.pos;
                self.blank();
                if self.peek() == Some(':') {
                    self.slice(Some(n))
                } else {
                    self.pos = before;
                    Ok(Selector::Index(n))
                }
            }
            _ => self.fail("expected a selector"),
        }
    }

    /// The rest of `start:end:step`, at the first `:`.
    fn slice(&mut self, start: Option<i64>) -> Parsed<Selector> {
        self.expect(":")?;
        self.blank();
        let end = self.optional_integer()?;
        self.blank();
        let step = if self.eat(":") {
            self.blank();
            self.optional_integer()?
        } else {
            None
        };
        Ok(Selector::Slice { start, end, step })
    }

    fn optional_integer(&mut self) -> Parsed<Option<i64>> {
        match self.peek() {
            Some(c) if c == '-' || c.is_ascii_digit() => Ok(Some(self.integer()?)),
            _ => Ok(None),
        }
    }

    /// `0`, or an optional `-` and digits without a leading zero.
    fn integer(&mut self) -> Parsed<i64> {
        let start = self.pos;
        let negative = self.eat("-");
        let digits_start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        let digits = &self.text[digits_start..self.pos];
        if digits.is_empty() || (digits.starts_with('0') && (digits.len() > 1 || negative)) {
            self.pos = start;
            return self.fail("expected an integer without leading zeros");
        }
        match digits.parse::<i64>() {
            Ok(n) if n <= MAX_INT => Ok(if negative { -n } else { n }),
            _ => {
                self.pos = start;
                self.fail("integers are limited to ±(2^53 - 1)")
            }
        }
    }

    /// A string in single or double quotes.
    fn string(&mut self) -> Parsed<String> {
        let quote = self.peek().expect("a quote");
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else {
                return self.fail("unterminated string");
            };
            self.pos += c.len_utf8();
            match c {
                _ if c == quote => return Ok(out),
                '\\' => out.push(self.escape(quote)?),
                '\u{0}'..='\u{1f}' => {
                    self.pos -= 1;
                    return self.fail("control characters must be escaped");
                }
                _ => out.push(c),
            }
        }
    }

    fn escape(&mut self, quote: char) -> Parsed<char> {
        let Some(c) = self.peek() else {
            return self.fail("unterminated escape");
        };
        self.pos += c.len_utf8();
        Ok(match c {
            'b' => '\u{8}',
            'f' => '\u{c}',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '/' => '/',
            '\\' => '\\',
            'u' => return self.unicode_escape(),
            _ if c == quote => c,
            _ => {
                self.pos -= c.len_utf8();
                return self.fail("invalid escape");
            }
        })
    }

    /// `\uXXXX`, or a surrogate pair of them.
    fn unicode_escape(&mut self) -> Parsed<char> {
        let high = self.hex4()?;
        if (0xDC00..=0xDFFF).contains(&high) {
            return self.fail("a low surrogate without a high one");
        }
        if !(0xD800..=0xDBFF).contains(&high) {
            return char::from_u32(high).map_or_else(|| self.fail("invalid code point"), Ok);
        }
        if !self.eat("\\u") {
            return self.fail("a high surrogate must be followed by a low one");
        }
        let low = self.hex4()?;
        if !(0xDC00..=0xDFFF).contains(&low) {
            return self.fail("a high surrogate must be followed by a low one");
        }
        let code = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
        char::from_u32(code).map_or_else(|| self.fail("invalid code point"), Ok)
    }

    fn hex4(&mut self) -> Parsed<u32> {
        let value = self
            .rest()
            .get(..4)
            .filter(|d| d.chars().all(|c| c.is_ascii_hexdigit()))
            .map(|d| u32::from_str_radix(d, 16).expect("hexadecimal digits"));
        let Some(value) = value else {
            return self.fail("expected four hexadecimal digits");
        };
        self.pos += 4;
        Ok(value)
    }

    // Filter expressions.

    fn logical(&mut self, depth: usize) -> Parsed<Logical> {
        if depth > MAX_DEPTH {
            return self.fail("the expression nests too deeply");
        }
        let mut terms = vec![self.and(depth)?];
        loop {
            let before = self.pos;
            self.blank();
            if self.eat("||") {
                self.blank();
                terms.push(self.and(depth)?);
            } else {
                self.pos = before;
                break;
            }
        }
        Ok(if terms.len() == 1 {
            terms.remove(0)
        } else {
            Logical::Or(terms)
        })
    }

    fn and(&mut self, depth: usize) -> Parsed<Logical> {
        let mut terms = vec![self.basic(depth)?];
        loop {
            let before = self.pos;
            self.blank();
            if self.eat("&&") {
                self.blank();
                terms.push(self.basic(depth)?);
            } else {
                self.pos = before;
                break;
            }
        }
        Ok(if terms.len() == 1 {
            terms.remove(0)
        } else {
            Logical::And(terms)
        })
    }

    /// A parenthesized expression, a comparison or a test, possibly
    /// negated.
    fn basic(&mut self, depth: usize) -> Parsed<Logical> {
        if self.eat("!") {
            self.blank();
            let inner = if self.peek() == Some('(') {
                self.parenthesized(depth)?
            } else {
                let start = self.pos;
                match self.operand(depth)? {
                    Operand::Query(query) => Logical::Exists(query),
                    Operand::Function(f) => self.test_function(f, start)?,
                    Operand::Literal(_) => {
                        self.pos = start;
                        return self.fail("`!` applies to a query, a function or `(...)`");
                    }
                }
            };
            return Ok(Logical::Not(Box::new(inner)));
        }
        if self.peek() == Some('(') {
            return self.parenthesized(depth);
        }
        let start = self.pos;
        let left = self.operand(depth)?;
        let before = self.pos;
        self.blank();
        if let Some(op) = self.comparison_op() {
            self.blank();
            let right_start = self.pos;
            let right = self.operand(depth)?;
            let left = Self::comparable(left, start)?;
            let right = Self::comparable(right, right_start)?;
            return Ok(Logical::Comparison(Box::new(left), op, Box::new(right)));
        }
        self.pos = before;
        match left {
            Operand::Query(query) => Ok(Logical::Exists(query)),
            Operand::Function(f) => self.test_function(f, start),
            Operand::Literal(_) => {
                self.pos = start;
                self.fail("a literal must be compared with something")
            }
        }
    }

    fn parenthesized(&mut self, depth: usize) -> Parsed<Logical> {
        self.expect("(")?;
        self.blank();
        let inner = self.logical(depth + 1)?;
        self.blank();
        self.expect(")")?;
        Ok(inner)
    }

    fn comparison_op(&mut self) -> Option<CompareOp> {
        for (token, op) in [
            ("==", CompareOp::Eq),
            ("!=", CompareOp::Ne),
            ("<=", CompareOp::Le),
            (">=", CompareOp::Ge),
            ("<", CompareOp::Lt),
            (">", CompareOp::Gt),
        ] {
            if self.eat(token) {
                return Some(op);
            }
        }
        None
    }

    /// A function used as a test must return a logical value.
    fn test_function(&mut self, f: Function, start: usize) -> Parsed<Logical> {
        if f.name.result() == Type::Logical {
            Ok(Logical::Function(f))
        } else {
            self.pos = start;
            self.fail(&format!(
                "{}() returns a value, which must be compared",
                f.name.as_str()
            ))
        }
    }

    /// One side of a comparison: a literal, a singular query or a function
    /// returning a value.
    fn comparable(operand: Operand, start: usize) -> Parsed<Comparable> {
        let fail = |message: &str| {
            Err(ParseError {
                message: message.to_owned(),
                offset: start,
            })
        };
        match operand {
            Operand::Literal(l) => Ok(Comparable::Literal(l)),
            Operand::Query(q) if q.is_singular() => Ok(Comparable::Query(q)),
            Operand::Query(_) => fail("only queries that select at most one node can be compared"),
            Operand::Function(f) if f.name.result() == Type::Value => Ok(Comparable::Function(f)),
            Operand::Function(f) => fail(&format!(
                "{}() returns a logical value, which cannot be compared",
                f.name.as_str()
            )),
        }
    }

    fn operand(&mut self, depth: usize) -> Parsed<Operand> {
        match self.peek() {
            Some('$' | '@') => Ok(Operand::Query(self.query(depth + 1)?)),
            Some('\'' | '"') => Ok(Operand::Literal(Literal::String(self.string()?))),
            Some(c) if c == '-' || c.is_ascii_digit() => Ok(Operand::Literal(self.number()?)),
            Some(c) if c.is_ascii_lowercase() => {
                for (word, literal) in [
                    ("true", Literal::Bool(true)),
                    ("false", Literal::Bool(false)),
                    ("null", Literal::Null),
                ] {
                    let Some(after) = self.rest().strip_prefix(word) else {
                        continue;
                    };
                    let follows = after.chars().next();
                    if !follows.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '(') {
                        self.pos += word.len();
                        return Ok(Operand::Literal(literal));
                    }
                }
                Ok(Operand::Function(self.function(depth)?))
            }
            _ => self.fail("expected a query, a literal or a function"),
        }
    }

    /// `(int / "-0") [frac] [exp]`.
    fn number(&mut self) -> Parsed<Literal> {
        let start = self.pos;
        self.eat("-");
        let digits = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        let int = &self.text[digits..self.pos];
        if int.is_empty() || (int.starts_with('0') && int.len() > 1) {
            self.pos = start;
            return self.fail("expected a number without leading zeros");
        }
        if self.eat(".") {
            let frac = self.pos;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
            if self.pos == frac {
                return self.fail("expected digits after `.`");
            }
        }
        if self.eat("e") || self.eat("E") {
            if !self.eat("-") {
                self.eat("+");
            }
            let exp = self.pos;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
            if self.pos == exp {
                return self.fail("expected digits in the exponent");
            }
        }
        let text = &self.text[start..self.pos];
        if let Ok(d) = Decimal::parse(text) {
            Ok(Literal::Number(d))
        } else {
            self.pos = start;
            self.fail("the number is out of range")
        }
    }

    /// `name(arguments)`, checked against the function's parameters.
    fn function(&mut self, depth: usize) -> Parsed<Function> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            self.pos += 1;
        }
        let name_text = &self.text[start..self.pos];
        let Some(name) = FunctionName::parse(name_text) else {
            self.pos = start;
            return self.fail(&format!("unknown function {name_text}()"));
        };
        self.expect("(")?;
        self.blank();
        let mut arguments = Vec::new();
        if !self.eat(")") {
            loop {
                let at = self.pos;
                arguments.push((self.argument(depth + 1)?, at));
                self.blank();
                if self.eat(")") {
                    break;
                }
                self.expect(",")?;
                self.blank();
            }
        }
        let parameters = name.parameters();
        if arguments.len() != parameters.len() {
            self.pos = start;
            return self.fail(&format!(
                "{name_text}() takes {} argument{}",
                parameters.len(),
                if parameters.len() == 1 { "" } else { "s" }
            ));
        }
        let mut checked = Vec::with_capacity(arguments.len());
        for ((argument, at), parameter) in arguments.into_iter().zip(parameters) {
            checked.push(check_argument(argument, *parameter, at)?);
        }
        Ok(Function {
            name,
            arguments: checked,
        })
    }

    /// A function argument: a literal, a query, a logical expression or a
    /// function.
    fn argument(&mut self, depth: usize) -> Parsed<Argument> {
        if matches!(self.peek(), Some('(' | '!')) {
            return Ok(Argument::Logical(self.logical(depth)?));
        }
        let start = self.pos;
        let operand = self.operand(depth)?;
        let before = self.pos;
        self.blank();
        let continues = ["==", "!=", "<", ">", "&&", "||"]
            .iter()
            .any(|t| self.rest().starts_with(t));
        self.pos = before;
        if continues {
            self.pos = start;
            return Ok(Argument::Logical(self.logical(depth)?));
        }
        Ok(match operand {
            Operand::Literal(l) => Argument::Literal(l),
            Operand::Query(q) => Argument::Query(q),
            Operand::Function(f) => Argument::Function(f),
        })
    }
}

enum Operand {
    Literal(Literal),
    Query(Query),
    Function(Function),
}

/// Whether `argument` can be passed as `parameter`, per RFC 9535's type
/// rules.
fn check_argument(argument: Argument, parameter: Type, at: usize) -> Parsed<Argument> {
    let ok = match (&argument, parameter) {
        (Argument::Literal(_), Type::Value)
        | (Argument::Query(_), Type::Nodes | Type::Logical)
        | (Argument::Logical(_), Type::Logical) => true,
        (Argument::Query(q), Type::Value) => q.is_singular(),
        (Argument::Function(f), Type::Value | Type::Logical) => f.name.result() == parameter,
        _ => false,
    };
    if ok {
        Ok(argument)
    } else {
        let expected = match parameter {
            Type::Value => "a literal, a query that selects at most one node, or a value",
            Type::Logical => "a logical expression",
            Type::Nodes => "a query",
        };
        Err(ParseError {
            message: format!("expected {expected}"),
            offset: at,
        })
    }
}

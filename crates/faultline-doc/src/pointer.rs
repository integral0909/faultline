//! JSON Pointer (RFC 6901), including its URI fragment representation.

use std::fmt;

use crate::uri::{percent_decode, percent_encode_fragment};

/// A parsed JSON Pointer: a sequence of unescaped reference tokens.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JsonPointer {
    tokens: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PointerError {
    /// A non-empty pointer must start with `/`.
    MissingLeadingSlash,
    /// `~` must be followed by `0` or `1`.
    InvalidEscape { position: usize },
    /// The fragment contained malformed percent-encoding or decoded to
    /// invalid UTF-8.
    InvalidPercentEncoding,
}

impl fmt::Display for PointerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PointerError::MissingLeadingSlash => {
                f.write_str("a JSON Pointer must be empty or start with '/'")
            }
            PointerError::InvalidEscape { position } => write!(
                f,
                "invalid escape at position {position}: '~' must be followed by '0' or '1'"
            ),
            PointerError::InvalidPercentEncoding => {
                f.write_str("invalid percent-encoding in URI fragment")
            }
        }
    }
}

impl std::error::Error for PointerError {}

impl JsonPointer {
    /// The empty pointer, which refers to the whole document.
    pub fn root() -> Self {
        Self::default()
    }

    /// Parses the string representation, e.g. `/paths/~1pets/get`.
    pub fn parse(text: &str) -> Result<Self, PointerError> {
        if text.is_empty() {
            return Ok(Self::root());
        }
        let Some(rest) = text.strip_prefix('/') else {
            return Err(PointerError::MissingLeadingSlash);
        };
        let mut tokens = Vec::new();
        let mut offset = 1;
        for raw in rest.split('/') {
            tokens.push(unescape(raw, offset)?);
            offset += raw.len() + 1;
        }
        Ok(Self { tokens })
    }

    /// Parses the URI fragment representation (without the leading `#`), e.g.
    /// `/paths/~1pets%7Bid%7D`. Percent-decoding happens before `~` unescaping,
    /// as RFC 6901 §6 requires.
    pub fn from_uri_fragment(fragment: &str) -> Result<Self, PointerError> {
        let decoded = percent_decode(fragment).ok_or(PointerError::InvalidPercentEncoding)?;
        Self::parse(&decoded)
    }

    pub fn from_tokens<I, S>(tokens: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            tokens: tokens.into_iter().map(Into::into).collect(),
        }
    }

    pub fn tokens(&self) -> &[String] {
        &self.tokens
    }

    pub fn is_root(&self) -> bool {
        self.tokens.is_empty()
    }

    /// A new pointer with `token` appended.
    #[must_use]
    pub fn join(&self, token: impl Into<String>) -> Self {
        let mut tokens = self.tokens.clone();
        tokens.push(token.into());
        Self { tokens }
    }

    pub fn push(&mut self, token: impl Into<String>) {
        self.tokens.push(token.into());
    }

    /// The pointer to the parent location, or `None` for the root.
    pub fn parent(&self) -> Option<Self> {
        let (_, parent) = self.tokens.split_last()?;
        Some(Self {
            tokens: parent.to_vec(),
        })
    }

    /// Whether `self` is `other` or one of its ancestors.
    pub fn is_prefix_of(&self, other: &JsonPointer) -> bool {
        other.tokens.starts_with(&self.tokens)
    }

    /// The fragment form, percent-encoded for use after `#` in a URI.
    pub fn to_uri_fragment(&self) -> String {
        percent_encode_fragment(&self.to_string())
    }
}

fn unescape(raw: &str, offset: usize) -> Result<String, PointerError> {
    if !raw.contains('~') {
        return Ok(raw.to_owned());
    }
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '~' {
            match chars.next() {
                Some((_, '0')) => out.push('~'),
                Some((_, '1')) => out.push('/'),
                _ => {
                    return Err(PointerError::InvalidEscape {
                        position: offset + i,
                    });
                }
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

impl fmt::Display for JsonPointer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for token in &self.tokens {
            f.write_str("/")?;
            // Order matters: escaping '~' first keeps '~1' produced from '/'
            // from being re-escaped.
            f.write_str(&token.replace('~', "~0").replace('/', "~1"))?;
        }
        Ok(())
    }
}

impl fmt::Debug for JsonPointer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JsonPointer({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn rfc6901_examples_parse_to_expected_tokens() {
        let cases: [(&str, &[&str]); 11] = [
            ("", &[]),
            ("/foo", &["foo"]),
            ("/foo/0", &["foo", "0"]),
            ("/", &[""]),
            ("/a~1b", &["a/b"]),
            ("/c%d", &["c%d"]),
            ("/e^f", &["e^f"]),
            ("/g|h", &["g|h"]),
            ("/i\\j", &["i\\j"]),
            ("/ ", &[" "]),
            ("/m~0n", &["m~n"]),
        ];
        for (text, tokens) in cases {
            assert_eq!(
                JsonPointer::parse(text).unwrap().tokens(),
                tokens,
                "{text:?}"
            );
        }
    }

    #[test]
    fn fragment_form_is_percent_decoded_first() {
        let p = JsonPointer::from_uri_fragment("/c%25d/k%22l/~01").unwrap();
        assert_eq!(p.tokens(), ["c%d", "k\"l", "~1"]);
        assert_eq!(
            JsonPointer::from_uri_fragment("/%E2%82%AC")
                .unwrap()
                .tokens(),
            ["€"]
        );
        assert_eq!(
            JsonPointer::from_uri_fragment("/%ZZ"),
            Err(PointerError::InvalidPercentEncoding)
        );
        assert_eq!(
            JsonPointer::from_uri_fragment("/%FF"),
            Err(PointerError::InvalidPercentEncoding)
        );
    }

    #[test]
    fn errors_report_position() {
        assert_eq!(
            JsonPointer::parse("foo"),
            Err(PointerError::MissingLeadingSlash)
        );
        assert_eq!(
            JsonPointer::parse("/ok/a~2"),
            Err(PointerError::InvalidEscape { position: 5 })
        );
        assert_eq!(
            JsonPointer::parse("/~"),
            Err(PointerError::InvalidEscape { position: 1 })
        );
    }

    #[test]
    fn display_escapes_and_fragment_encodes() {
        let p = JsonPointer::from_tokens(["paths", "/pets/{id}", "a~b"]);
        assert_eq!(p.to_string(), "/paths/~1pets~1{id}/a~0b");
        assert_eq!(p.to_uri_fragment(), "/paths/~1pets~1%7Bid%7D/a~0b");
    }

    #[test]
    fn parent_and_prefix() {
        let p = JsonPointer::parse("/a/b").unwrap();
        assert_eq!(p.parent().unwrap().to_string(), "/a");
        assert!(JsonPointer::root().is_prefix_of(&p));
        assert!(p.parent().unwrap().is_prefix_of(&p));
        assert!(!p.is_prefix_of(&p.parent().unwrap()));
        assert_eq!(JsonPointer::root().parent(), None);
    }

    proptest! {
        #[test]
        fn string_form_round_trips(tokens in prop::collection::vec("[a-z~/%{} ]{0,6}", 0..5)) {
            let p = JsonPointer::from_tokens(tokens);
            prop_assert_eq!(JsonPointer::parse(&p.to_string()).unwrap(), p.clone());
            prop_assert_eq!(JsonPointer::from_uri_fragment(&p.to_uri_fragment()).unwrap(), p);
        }
    }
}

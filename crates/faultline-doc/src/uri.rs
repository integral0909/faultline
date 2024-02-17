//! URI references (RFC 3986): parsing, resolution against a base, and
//! normalization for comparison.
//!
//! JSON Schema identifies schemas by URI (`$id`, `$ref`, `$anchor`), OpenAPI
//! references files by relative URI, and resolution must follow RFC 3986
//! exactly for identifiers to match. General-purpose URL crates implement the
//! WHATWG URL standard instead, which rejects relative bases and rewrites
//! some inputs, so Faultline carries its own small implementation.
//!
//! Parsing is deliberately lenient about characters that should have been
//! percent-encoded (`{`, spaces, ...), because real-world `$ref` values contain
//! them unencoded; only control characters are rejected.

use std::fmt::{self, Write as _};

/// A parsed URI reference: either an absolute URI or a relative reference.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UriRef {
    scheme: Option<String>,
    authority: Option<String>,
    path: String,
    query: Option<String>,
    fragment: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UriError {
    ControlCharacter { position: usize },
}

impl fmt::Display for UriError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UriError::ControlCharacter { position } => {
                write!(f, "URI contains a control character at position {position}")
            }
        }
    }
}

impl std::error::Error for UriError {}

impl UriRef {
    /// Splits `text` into components following RFC 3986 Appendix B.
    pub fn parse(text: &str) -> Result<Self, UriError> {
        if let Some(position) = text.find(char::is_control) {
            return Err(UriError::ControlCharacter { position });
        }

        let (rest, fragment) = match text.split_once('#') {
            Some((rest, fragment)) => (rest, Some(fragment.to_owned())),
            None => (text, None),
        };
        let (rest, query) = match rest.split_once('?') {
            Some((rest, query)) => (rest, Some(query.to_owned())),
            None => (rest, None),
        };

        let (scheme, rest) = match rest.find([':', '/']) {
            Some(i) if rest.as_bytes()[i] == b':' && is_scheme(&rest[..i]) => {
                (Some(rest[..i].to_owned()), &rest[i + 1..])
            }
            _ => (None, rest),
        };

        let (authority, path) = match rest.strip_prefix("//") {
            Some(after) => {
                let end = after.find('/').unwrap_or(after.len());
                (Some(after[..end].to_owned()), &after[end..])
            }
            None => (None, rest),
        };

        Ok(Self {
            scheme,
            authority,
            path: path.to_owned(),
            query,
            fragment,
        })
    }

    pub fn scheme(&self) -> Option<&str> {
        self.scheme.as_deref()
    }

    pub fn authority(&self) -> Option<&str> {
        self.authority.as_deref()
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }

    /// Whether the reference has a scheme, i.e. does not depend on a base.
    pub fn has_scheme(&self) -> bool {
        self.scheme.is_some()
    }

    /// Whether this reference consists of a fragment only (`#...`) and
    /// therefore points into the current document.
    pub fn is_fragment_only(&self) -> bool {
        self.scheme.is_none()
            && self.authority.is_none()
            && self.path.is_empty()
            && self.query.is_none()
            && self.fragment.is_some()
    }

    #[must_use]
    pub fn without_fragment(&self) -> Self {
        Self {
            fragment: None,
            ..self.clone()
        }
    }

    #[must_use]
    pub fn with_fragment(&self, fragment: Option<&str>) -> Self {
        Self {
            fragment: fragment.map(str::to_owned),
            ..self.clone()
        }
    }

    /// Resolves `reference` against `self` as the base (RFC 3986 §5.2.2).
    #[must_use]
    pub fn resolve(&self, reference: &UriRef) -> UriRef {
        let base = self;
        let r = reference;
        let (scheme, authority, path, query);
        if r.scheme.is_some() {
            scheme = r.scheme.clone();
            authority = r.authority.clone();
            path = remove_dot_segments(&r.path);
            query = r.query.clone();
        } else {
            if r.authority.is_some() {
                authority = r.authority.clone();
                path = remove_dot_segments(&r.path);
                query = r.query.clone();
            } else {
                if r.path.is_empty() {
                    path = base.path.clone();
                    query = r.query.clone().or_else(|| base.query.clone());
                } else {
                    path = if r.path.starts_with('/') {
                        remove_dot_segments(&r.path)
                    } else {
                        remove_dot_segments(&merge(base, &r.path))
                    };
                    query = r.query.clone();
                }
                authority = base.authority.clone();
            }
            scheme = base.scheme.clone();
        }
        UriRef {
            scheme,
            authority,
            path,
            query,
            fragment: r.fragment.clone(),
        }
    }

    /// A normalized copy suitable for comparing identifiers (RFC 3986 §6.2.2
    /// syntax-based normalization plus default-port removal for http/https).
    #[must_use]
    pub fn normalized(&self) -> UriRef {
        let scheme = self.scheme.as_ref().map(|s| s.to_ascii_lowercase());
        let authority = self.authority.as_ref().map(|a| {
            let (userinfo, host_port) = match a.rsplit_once('@') {
                Some((user, host)) => (Some(user), host),
                None => (None, a.as_str()),
            };
            // Decode first, then lowercase: decoding `%44` yields `D`, which
            // must also be lowercased for normalization to be idempotent.
            let mut host_port = lowercase_outside_escapes(&normalize_percent(host_port));
            let default_port = match scheme.as_deref() {
                Some("http") => Some(":80"),
                Some("https") => Some(":443"),
                _ => None,
            };
            if let Some(port) = default_port
                && host_port.ends_with(port)
            {
                host_port.truncate(host_port.len() - port.len());
            }
            match userinfo {
                Some(user) => format!("{}@{host_port}", normalize_percent(user)),
                None => host_port,
            }
        });
        let mut path = remove_dot_segments(&normalize_percent(&self.path));
        if path.is_empty() && authority.is_some() {
            path.push('/');
        }
        UriRef {
            scheme,
            authority,
            path,
            query: self.query.as_deref().map(normalize_percent),
            fragment: self.fragment.as_deref().map(normalize_percent),
        }
    }
}

fn is_scheme(s: &str) -> bool {
    let mut bytes = s.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

/// RFC 3986 §5.2.3.
fn merge(base: &UriRef, reference_path: &str) -> String {
    if base.authority.is_some() && base.path.is_empty() {
        format!("/{reference_path}")
    } else {
        match base.path.rfind('/') {
            Some(i) => format!("{}{reference_path}", &base.path[..=i]),
            None => reference_path.to_owned(),
        }
    }
}

/// RFC 3986 §5.2.4, operating on whole segments rather than repeatedly
/// rewriting the input buffer.
fn remove_dot_segments(path: &str) -> String {
    let mut input = path;
    let mut output: Vec<&str> = Vec::new();
    // Each output entry is a segment *including* its leading '/', except
    // possibly the first, which preserves relative paths like "a/b".
    while !input.is_empty() {
        if let Some(rest) = input.strip_prefix("../") {
            input = rest;
        } else if let Some(rest) = input.strip_prefix("./") {
            input = rest;
        } else if input.starts_with("/./") {
            input = &input[2..];
        } else if input == "/." {
            input = "/";
        } else if input.starts_with("/../") {
            input = &input[3..];
            output.pop();
        } else if input == "/.." {
            input = "/";
            output.pop();
        } else if input == "." || input == ".." {
            input = "";
        } else {
            let start = usize::from(input.starts_with('/'));
            let end = input[start..].find('/').map_or(input.len(), |i| i + start);
            output.push(&input[..end]);
            input = &input[end..];
        }
    }
    let joined = output.concat();
    // The RFC algorithm is specified for absolute paths; when `..` consumes the
    // first segment of a relative path the next segment keeps its '/', which
    // would turn "a/../b" into "/b". Keep relative paths relative.
    if !path.starts_with('/') && joined.starts_with('/') {
        joined[1..].to_owned()
    } else {
        joined
    }
}

impl fmt::Display for UriRef {
    /// Recomposes the reference (RFC 3986 §5.3).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(scheme) = &self.scheme {
            write!(f, "{scheme}:")?;
        }
        if let Some(authority) = &self.authority {
            write!(f, "//{authority}")?;
        }
        f.write_str(&self.path)?;
        if let Some(query) = &self.query {
            write!(f, "?{query}")?;
        }
        if let Some(fragment) = &self.fragment {
            write!(f, "#{fragment}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for UriRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UriRef({self})")
    }
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

/// Decodes `%XX` sequences. Returns `None` for malformed escapes or when the
/// decoded bytes are not UTF-8.
pub fn percent_decode(text: &str) -> Option<String> {
    if !text.contains('%') {
        return Some(text.to_owned());
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = hex_value(*bytes.get(i + 1)?)?;
            let lo = hex_value(*bytes.get(i + 2)?)?;
            out.push(hi << 4 | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Percent-encodes everything that may not appear literally in a URI
/// fragment (RFC 3986 §3.5), including `%` itself.
pub fn percent_encode_fragment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for &b in text.as_bytes() {
        let allowed = is_unreserved(b)
            || matches!(
                b,
                b'!' | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
                    | b'/'
                    | b'?'
            );
        if allowed {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Uppercases percent-escape hex digits and decodes escapes of unreserved
/// characters (RFC 3986 §6.2.2.1–2).
///
/// A `%` that does not start a valid escape is a literal percent sign and is
/// re-encoded as `%25`. Copying it through unchanged would make normalization
/// non-idempotent: in `%6%66`, decoding `%66` to `f` would create a new
/// escape `%6f` for the next pass to decode.
fn normalize_percent(text: &str) -> String {
    if !text.contains('%') {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(hi), Some(lo)) = (
                bytes.get(i + 1).copied().and_then(hex_value),
                bytes.get(i + 2).copied().and_then(hex_value),
            )
        {
            let value = hi << 4 | lo;
            if is_unreserved(value) {
                out.push(value as char);
            } else {
                let _ = write!(out, "%{value:02X}");
            }
            i += 3;
            continue;
        }
        if bytes[i] == b'%' {
            out.push_str("%25");
            i += 1;
            continue;
        }
        // Copy one whole UTF-8 character.
        let ch_len = text[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&text[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// ASCII-lowercases `text` except for the hex digits of percent escapes,
/// which normalized form keeps uppercase. Expects output of
/// [`normalize_percent`], where every `%` starts a valid escape.
fn lowercase_outside_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        out.push(c.to_ascii_lowercase());
        if c == '%' {
            out.extend(chars.by_ref().take(2));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> UriRef {
        UriRef::parse(s).unwrap()
    }

    fn resolve(base: &str, r: &str) -> String {
        u(base).resolve(&u(r)).to_string()
    }

    const BASE: &str = "http://a/b/c/d;p?q";

    #[test]
    fn rfc3986_normal_examples() {
        let cases = [
            ("g:h", "g:h"),
            ("g", "http://a/b/c/g"),
            ("./g", "http://a/b/c/g"),
            ("g/", "http://a/b/c/g/"),
            ("/g", "http://a/g"),
            ("//g", "http://g"),
            ("?y", "http://a/b/c/d;p?y"),
            ("g?y", "http://a/b/c/g?y"),
            ("#s", "http://a/b/c/d;p?q#s"),
            ("g#s", "http://a/b/c/g#s"),
            ("g?y#s", "http://a/b/c/g?y#s"),
            (";x", "http://a/b/c/;x"),
            ("g;x", "http://a/b/c/g;x"),
            ("g;x?y#s", "http://a/b/c/g;x?y#s"),
            ("", "http://a/b/c/d;p?q"),
            (".", "http://a/b/c/"),
            ("./", "http://a/b/c/"),
            ("..", "http://a/b/"),
            ("../", "http://a/b/"),
            ("../g", "http://a/b/g"),
            ("../..", "http://a/"),
            ("../../", "http://a/"),
            ("../../g", "http://a/g"),
        ];
        for (r, expected) in cases {
            assert_eq!(resolve(BASE, r), expected, "{r:?}");
        }
    }

    #[test]
    fn rfc3986_abnormal_examples() {
        let cases = [
            ("../../../g", "http://a/g"),
            ("../../../../g", "http://a/g"),
            ("/./g", "http://a/g"),
            ("/../g", "http://a/g"),
            ("g.", "http://a/b/c/g."),
            (".g", "http://a/b/c/.g"),
            ("g..", "http://a/b/c/g.."),
            ("..g", "http://a/b/c/..g"),
            ("./../g", "http://a/b/g"),
            ("./g/.", "http://a/b/c/g/"),
            ("g/./h", "http://a/b/c/g/h"),
            ("g/../h", "http://a/b/c/h"),
            ("g;x=1/./y", "http://a/b/c/g;x=1/y"),
            ("g;x=1/../y", "http://a/b/c/y"),
            ("g?y/./x", "http://a/b/c/g?y/./x"),
            ("g?y/../x", "http://a/b/c/g?y/../x"),
            ("g#s/./x", "http://a/b/c/g#s/./x"),
            ("g#s/../x", "http://a/b/c/g#s/../x"),
            ("http:g", "http:g"),
        ];
        for (r, expected) in cases {
            assert_eq!(resolve(BASE, r), expected, "{r:?}");
        }
    }

    #[test]
    fn relative_bases_resolve_like_file_paths() {
        assert_eq!(resolve("schemas/pet.json", "tag.json"), "schemas/tag.json");
        assert_eq!(
            resolve("schemas/pet.json", "../common.yaml#/x"),
            "common.yaml#/x"
        );
        assert_eq!(resolve("pet.json", "#/defs/a"), "pet.json#/defs/a");
        assert_eq!(
            resolve("file:///ws/api/openapi.yaml", "./s/pet.yaml"),
            "file:///ws/api/s/pet.yaml"
        );
    }

    #[test]
    fn components_are_split_per_appendix_b() {
        let r = u("https://user@example.com:8080/a/b?x=1#frag/ment");
        assert_eq!(r.scheme(), Some("https"));
        assert_eq!(r.authority(), Some("user@example.com:8080"));
        assert_eq!(r.path(), "/a/b");
        assert_eq!(r.query(), Some("x=1"));
        assert_eq!(r.fragment(), Some("frag/ment"));
        assert!(u("#/definitions/Pet").is_fragment_only());
        assert!(!u("pet.json#/x").is_fragment_only());
        // A colon after a slash is part of the path, not a scheme.
        assert_eq!(u("./a:b").scheme(), None);
        // Unencoded braces are tolerated.
        assert_eq!(
            u("#/paths/~1pets~1{id}").fragment(),
            Some("/paths/~1pets~1{id}")
        );
        assert_eq!(
            UriRef::parse("a\nb"),
            Err(UriError::ControlCharacter { position: 1 })
        );
    }

    #[test]
    fn normalization_makes_equivalent_identifiers_equal() {
        let a = u("HTTP://Example.COM:80/a/./b/../c/%7euser/%3a");
        let b = u("http://example.com/a/c/~user/%3A");
        assert_eq!(a.normalized(), b.normalized());
        assert_eq!(u("https://x.io").normalized().to_string(), "https://x.io/");
        assert_ne!(
            u("https://x.io:8443/").normalized(),
            u("https://x.io/").normalized()
        );
    }

    #[test]
    fn normalization_is_idempotent_with_stray_percent_signs() {
        // Found by fuzzing.
        let once = u("/a%6%666").normalized();
        assert_eq!(once.path(), "/a%256f6");
        assert_eq!(once.normalized(), once);
        assert_eq!(u("/100%").normalized().path(), "/100%25");
        // Decoded unreserved characters in the host are lowercased too.
        let host = u("//%44%d4%%x/").normalized();
        assert_eq!(host.authority(), Some("d%D4%25%25x"));
        assert_eq!(host.normalized(), host);
    }

    #[test]
    fn percent_helpers() {
        assert_eq!(percent_decode("a%20b%e2%82%ac").as_deref(), Some("a b€"));
        assert_eq!(percent_decode("%2"), None);
        assert_eq!(
            percent_encode_fragment("/a b/{x}/€"),
            "/a%20b/%7Bx%7D/%E2%82%AC"
        );
    }
}

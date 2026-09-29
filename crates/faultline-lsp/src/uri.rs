//! `file:` URIs as editors send them, and the paths they name.

use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use faultline_doc::uri::percent_decode;

/// The local path a `file:` URI names, or `None` for other schemes and for
/// URIs naming a remote host.
pub(crate) fn to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri
        .get(..7)
        .filter(|scheme| scheme.eq_ignore_ascii_case("file://"))
        .map(|_| &uri[7..])?;
    let (authority, path) = rest.split_at(rest.find('/')?);
    if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let decoded = percent_decode(path)?;
    Some(native(&decoded))
}

#[cfg(windows)]
fn native(decoded: &str) -> PathBuf {
    // `/C:/Users/...` names `C:\Users\...`.
    let trimmed = match decoded.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => &decoded[1..],
        _ => decoded,
    };
    PathBuf::from(trimmed.replace('/', "\\"))
}

#[cfg(not(windows))]
fn native(decoded: &str) -> PathBuf {
    PathBuf::from(decoded)
}

/// The `file:` URI of an absolute path.
pub(crate) fn from_path(path: &Path) -> String {
    let mut uri = String::from("file://");
    let mut any = false;
    for component in path.components() {
        let part = match component {
            Component::Prefix(prefix) => {
                // `C:` (or a verbatim `\\?\C:`) becomes `/C:`.
                let text = prefix.as_os_str().to_string_lossy();
                let text = text.trim_start_matches(r"\\?\");
                uri.push('/');
                uri.push_str(text);
                any = true;
                continue;
            }
            Component::RootDir | Component::CurDir => continue,
            Component::ParentDir => "..".into(),
            Component::Normal(part) => part.to_string_lossy(),
        };
        uri.push('/');
        encode_into(&mut uri, &part);
        any = true;
    }
    if !any {
        uri.push('/');
    }
    uri
}

fn encode_into(out: &mut String, segment: &str) {
    for &b in segment.as_bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b'@'
            );
        if keep {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn round_trips() {
        for path in [
            "/work/api/openapi.yaml",
            "/work/api specs/pet#1.yaml",
            "/é/ü.json",
        ] {
            let uri = from_path(Path::new(path));
            assert_eq!(to_path(&uri), Some(PathBuf::from(path)), "{uri}");
        }
        assert_eq!(from_path(Path::new("/a b/c%d")), "file:///a%20b/c%25d");
        assert_eq!(from_path(Path::new("/")), "file:///");
    }

    #[cfg(not(windows))]
    #[test]
    fn decodes_editor_spellings() {
        assert_eq!(
            to_path("file:///Users/me/My%20API/pets.yaml"),
            Some(PathBuf::from("/Users/me/My API/pets.yaml"))
        );
        assert_eq!(
            to_path("FILE://localhost/tmp/a.proto"),
            Some(PathBuf::from("/tmp/a.proto"))
        );
        assert_eq!(to_path("file://server/share/a.proto"), None);
        assert_eq!(to_path("untitled:Untitled-1"), None);
        assert_eq!(to_path("https://example.com/a.yaml"), None);
        assert_eq!(to_path("file://"), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_drives() {
        assert_eq!(
            to_path("file:///c%3A/Users/me/api.yaml"),
            Some(PathBuf::from(r"c:\Users\me\api.yaml"))
        );
        assert_eq!(
            from_path(Path::new(r"C:\Users\me\api.yaml")),
            "file:///C:/Users/me/api.yaml"
        );
    }
}

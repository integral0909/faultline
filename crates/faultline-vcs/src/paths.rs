//! Resolving paths that may exist only in a revision.

use std::path::{Component, Path, PathBuf};

/// Removes `.` and resolves `..` components without touching the
/// filesystem. Returns `None` when `..` would climb above the start of a
/// relative path or above the root of an absolute one.
pub fn normalize_lexically(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => out.push(component),
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                out.pop();
            }
            Component::Normal(part) => {
                depth += 1;
                out.push(part);
            }
        }
    }
    Some(out)
}

/// Canonicalizes the longest prefix of an absolute path that exists, then
/// appends the rest lexically. Symbolic links in the existing part are
/// resolved, so the result can be compared with a canonical working tree;
/// the part that does not exist on disk cannot contain links.
pub(crate) fn resolve_existing_prefix(absolute: &Path) -> Option<PathBuf> {
    let normalized = normalize_lexically(absolute)?;
    let mut existing = normalized.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(canonical) = existing.canonicalize() {
            let mut out = canonical;
            out.extend(rest.iter().rev());
            return Some(out);
        }
        rest.push(existing.file_name()?.to_owned());
        existing = existing.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_normalization() {
        let n = |p: &str| normalize_lexically(Path::new(p));
        assert_eq!(n("a/./b/../c"), Some(PathBuf::from("a/c")));
        assert_eq!(n("/a/b/../../c"), Some(PathBuf::from("/c")));
        assert_eq!(n("a/../.."), None);
        assert_eq!(n("/.."), None);
    }

    #[test]
    fn missing_suffixes_are_appended_to_the_canonical_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = dir.path().canonicalize().unwrap();
        let missing = dir.path().join("gone/../also-gone/x.yaml");
        assert_eq!(
            resolve_existing_prefix(&missing).unwrap(),
            // Component by component: `/` does not separate a verbatim
            // (`\\?\`) Windows path.
            canonical.join("also-gone").join("x.yaml")
        );
    }
}

//! Semantic-version suggestions.

use std::fmt;

use faultline_diff::ChangeKind;

use crate::classify::{Finding, Severity};

/// The smallest version increment that honestly describes a set of changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bump {
    None,
    Patch,
    Minor,
    Major,
}

impl Bump {
    pub fn as_str(self) -> &'static str {
        match self {
            Bump::None => "none",
            Bump::Patch => "patch",
            Bump::Minor => "minor",
            Bump::Major => "major",
        }
    }
}

impl fmt::Display for Bump {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Major for any breaking finding; minor for additions and for risky
/// findings (which may break some consumers and deserve release notes);
/// patch for any other change.
pub fn suggest_bump<'a>(findings: impl IntoIterator<Item = &'a Finding>) -> Bump {
    let mut bump = Bump::None;
    for finding in findings {
        let this = match finding.severity {
            Severity::Breaking => Bump::Major,
            Severity::Risky => Bump::Minor,
            Severity::Info if is_addition(&finding.change.kind) => Bump::Minor,
            Severity::Info => Bump::Patch,
        };
        bump = bump.max(this);
    }
    bump
}

/// Whether a change adds something (an operation, a field, a value) rather
/// than altering or removing it.
pub fn is_addition(kind: &ChangeKind) -> bool {
    matches!(
        kind,
        ChangeKind::OperationAdded
            | ChangeKind::ParameterAdded { .. }
            | ChangeKind::RequestBodyAdded { .. }
            | ChangeKind::MediaTypeAdded { .. }
            | ChangeKind::ResponseAdded { .. }
            | ChangeKind::ResponseHeaderAdded { .. }
            | ChangeKind::SecurityRequirementAdded { .. }
            | ChangeKind::CallbackAdded { .. }
            | ChangeKind::WebhookAdded { .. }
            | ChangeKind::SchemaAdded { .. }
            | ChangeKind::FieldAdded { .. }
            | ChangeKind::EnumValueAdded { .. }
            | ChangeKind::VariantAdded { .. }
            | ChangeKind::DiscriminatorValueAdded { .. }
    )
}

/// A semantic version: `MAJOR.MINOR.PATCH`, optionally with a `v` prefix, a
/// pre-release and build metadata (which do not affect the bump).
/// `MAJOR.MINOR` is read as `MAJOR.MINOR.0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let core = text.split(['-', '+']).next()?;
        let mut parts = core.split('.');
        let number = |part: Option<&str>| -> Option<u64> {
            let part = part?;
            // No leading zeros, as SemVer requires.
            if part.is_empty() || (part.len() > 1 && part.starts_with('0')) {
                return None;
            }
            part.parse().ok()
        };
        let major = number(parts.next())?;
        let minor = number(parts.next())?;
        let patch = match parts.next() {
            Some(part) => number(Some(part))?,
            None => 0,
        };
        if parts.next().is_some() {
            return None;
        }
        Some(Version {
            major,
            minor,
            patch,
        })
    }

    /// The increment from `self` to `next`, as `SemVer` reads it. Below
    /// `1.0.0`, anything may break: a minor increment counts as major, and a patch
    /// increment as minor. A version that did not increase is no bump.
    pub fn bump_to(self, next: Version) -> Bump {
        if next <= self {
            return Bump::None;
        }
        let minor_up = next.major == self.major && next.minor > self.minor;
        if next.major > self.major || (self.major == 0 && minor_up) {
            Bump::Major
        } else if self.major == 0 || minor_up {
            Bump::Minor
        } else {
            Bump::Patch
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The bump two declared versions amount to, when both are semantic
/// versions.
pub fn declared_bump(old: &str, new: &str) -> Option<Bump> {
    Some(Version::parse(old)?.bump_to(Version::parse(new)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse() {
        let v = |s: &str| Version::parse(s);
        assert_eq!(v("1.2.3").unwrap().to_string(), "1.2.3");
        assert_eq!(v("v2.0").unwrap().to_string(), "2.0.0");
        assert_eq!(v("1.0.0-rc.1+build.5").unwrap().to_string(), "1.0.0");
        for bad in [
            "",
            "1",
            "01.2.3",
            "1.2.3.4",
            "one.two",
            "1..2",
            "2024-05-01",
        ] {
            assert!(v(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn declared_bumps() {
        let b = |old: &str, new: &str| declared_bump(old, new).unwrap();
        assert_eq!(b("1.2.3", "2.0.0"), Bump::Major);
        assert_eq!(b("1.2.3", "1.3.0"), Bump::Minor);
        assert_eq!(b("1.2.3", "1.2.4"), Bump::Patch);
        assert_eq!(b("1.2.3", "1.2.3"), Bump::None);
        assert_eq!(b("1.2.3", "1.1.0"), Bump::None);
        // Below 1.0 a minor increment may break.
        assert_eq!(b("0.3.1", "0.4.0"), Bump::Major);
        assert_eq!(b("0.3.1", "0.3.2"), Bump::Minor);
        assert_eq!(b("0.9.0", "1.0.0"), Bump::Major);
        assert_eq!(declared_bump("2024-05", "2024-06"), None);
    }
}

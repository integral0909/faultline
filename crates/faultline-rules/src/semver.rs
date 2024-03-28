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

fn is_addition(kind: &ChangeKind) -> bool {
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

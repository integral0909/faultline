//! The XML Schema front end.
//!
//! [`reader`] reads schema documents, following `include` and `import`
//! through the workspace loader, into the global [`model::Components`] of
//! a schema set, and [`lower`] lowers them into the IR.

mod builtins;
pub mod codes;
mod complex;
pub mod lower;
pub mod model;
pub mod reader;

pub use lower::{Lowerer, type_name};
pub use reader::{Reader, XS};

use faultline_doc::UriRef;
use faultline_doc::loader::Loader;
use faultline_ir::{Api, Role, SchemaRoot};
use faultline_source::{Diagnostics, Limits, SourceMap};

/// The result of lowering one version.
#[derive(Debug)]
pub struct Lowered {
    /// `None` when the schemas have errors.
    pub api: Option<Api>,
    pub diagnostics: Diagnostics,
}

/// Reads the schema at `entry` and what it includes and imports, and
/// lowers it into an [`Api`] named `name` whose schema roots are the global
/// elements, used in `role`.
pub fn lower_xsd(
    entry: &UriRef,
    loader: &dyn Loader,
    sources: &SourceMap,
    name: &str,
    role: Role,
    limits: Limits,
) -> Lowered {
    let mut reader = Reader::new(loader, sources, limits);
    reader.load(entry);
    let mut diagnostics = std::mem::take(&mut reader.diagnostics);
    if diagnostics.has_errors() {
        diagnostics.sort();
        return Lowered {
            api: None,
            diagnostics,
        };
    }
    let mut lowerer = Lowerer::new(&reader.components, name);
    lowerer.element_roots(role);
    diagnostics.extend(std::mem::take(&mut lowerer.diagnostics));
    diagnostics.sort();
    let api = (!diagnostics.has_errors()).then_some(lowerer.api);
    Lowered { api, diagnostics }
}

impl Lowerer<'_> {
    /// Every global element that can appear in a document (not abstract)
    /// becomes a schema root, named by its local name, or by its expanded
    /// name when two namespaces declare the same local name.
    pub fn element_roots(&mut self, role: Role) {
        let order = self.components.element_order.clone();
        for name in &order {
            let decl = self.components.elements[name].clone();
            if decl.abstract_ {
                continue;
            }
            let shared = order
                .iter()
                .filter(|other| other.local == name.local)
                .count()
                > 1;
            let root = if shared {
                name.to_string()
            } else {
                name.local.clone()
            };
            let ty = self.element_type(&decl);
            self.api.schemas.push(SchemaRoot {
                name: root,
                ty,
                role,
                origin: Some(decl.span),
            });
        }
    }
}

//! Smithy models in the JSON AST form (`{"smithy": "2.0", "shapes": ...}`),
//! as Smithy's build tools write them.
//!
//! The JSON AST names shapes by absolute id. It is read into the same
//! syntax trees as the IDL, one per namespace, so that resolution and
//! lowering are shared. Members, operations, services and resources name
//! their targets with `{"target": "ns#Name"}`, and `apply` shapes add
//! traits to others.

use std::collections::BTreeMap;

use faultline_doc::{Node as DocNode, Number, Value};
use faultline_source::{Diagnostic, Diagnostics, Span};

use crate::ast::{
    Apply, IoShape, Member, Model, Name, Node, NodeValue, OperationBody, Shape, ShapeKind,
    SimpleType, Trait,
};
use crate::codes;

/// Reads a JSON AST document into one model per namespace.
pub fn read(root: &DocNode, diagnostics: &mut Diagnostics) -> Vec<Model> {
    let mut reader = Reader { diagnostics };
    reader.models(root)
}

struct Reader<'d> {
    diagnostics: &'d mut Diagnostics,
}

impl Reader<'_> {
    fn error(&mut self, message: String, span: Span) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(codes::SYNTAX)
                .with_primary(span, ""),
        );
    }

    fn models(&mut self, root: &DocNode) -> Vec<Model> {
        let Some(version) = root.get("smithy").and_then(DocNode::as_str) else {
            self.error(
                "a Smithy JSON AST names its version in `smithy`".into(),
                root.span,
            );
            return Vec::new();
        };
        let version = version.to_owned();
        let Some(shapes) = root.get("shapes").and_then(DocNode::as_object) else {
            return Vec::new();
        };
        let mut models: BTreeMap<String, Model> = BTreeMap::new();
        for (id, member) in shapes.members() {
            let Some((namespace, local)) = id.split_once('#') else {
                self.error(format!("{id} is not an absolute shape id"), member.key_span);
                continue;
            };
            let model = models.entry(namespace.to_owned()).or_insert_with(|| Model {
                version: Some(version.clone()),
                namespace: Some(Name {
                    text: namespace.to_owned(),
                    span: member.key_span,
                }),
                ..Model::default()
            });
            let name = Name {
                text: local.to_owned(),
                span: member.key_span,
            };
            let value = &member.value;
            let Some(kind) = value.get("type").and_then(DocNode::as_str) else {
                self.error(format!("shape {id} has no `type`"), member.key_span);
                continue;
            };
            if kind == "apply" {
                model.applies.push(Apply {
                    target: Name {
                        text: id.to_owned(),
                        span: member.key_span,
                    },
                    traits: Self::traits(value),
                });
                continue;
            }
            if let Some(shape) = self.shape(kind, name, value) {
                model.shapes.push(shape);
            }
        }
        models.into_values().collect()
    }

    fn shape(&mut self, kind: &str, name: Name, value: &DocNode) -> Option<Shape> {
        let kind = match kind {
            "enum" => ShapeKind::Enum,
            "intEnum" => ShapeKind::IntEnum,
            "list" => ShapeKind::List,
            "map" => ShapeKind::Map,
            "structure" => ShapeKind::Structure,
            "union" => ShapeKind::Union,
            "service" => ShapeKind::Service,
            "resource" => ShapeKind::Resource,
            "operation" => ShapeKind::Operation,
            other => {
                if let Some(simple) = SimpleType::parse(other) {
                    ShapeKind::Simple(simple)
                } else {
                    self.error(format!("unknown shape type {other:?}"), name.span);
                    return None;
                }
            }
        };
        let mut shape = Shape {
            kind,
            traits: Self::traits(value),
            doc: None,
            mixins: self.targets(value.get("mixins")),
            for_resource: None,
            members: Vec::new(),
            properties: None,
            operation: None,
            name,
        };
        match kind {
            ShapeKind::List => {
                shape.members = self.named_members(value, &["member"]);
            }
            ShapeKind::Map => {
                shape.members = self.named_members(value, &["key", "value"]);
            }
            ShapeKind::Structure | ShapeKind::Union | ShapeKind::Enum | ShapeKind::IntEnum => {
                shape.members = self.members(value.get("members"));
            }
            ShapeKind::Operation => {
                shape.operation = Some(OperationBody {
                    input: self.target(value.get("input")).map(IoShape::Target),
                    output: self.target(value.get("output")).map(IoShape::Target),
                    errors: self.targets(value.get("errors")),
                });
            }
            ShapeKind::Service | ShapeKind::Resource => {
                shape.properties = Some(self.properties(value));
            }
            ShapeKind::Simple(_) => {}
        }
        Some(shape)
    }

    /// `{"target": "ns#Name"}`.
    fn target(&mut self, node: Option<&DocNode>) -> Option<Name> {
        let node = node?;
        let Some(target) = node.get("target").and_then(DocNode::as_str) else {
            self.error("expected `{\"target\": ...}`".into(), node.span);
            return None;
        };
        Some(Name {
            text: target.to_owned(),
            span: node.span,
        })
    }

    fn targets(&mut self, node: Option<&DocNode>) -> Vec<Name> {
        let Some(items) = node.and_then(DocNode::as_array) else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|item| self.target(Some(item)))
            .collect()
    }

    fn traits(value: &DocNode) -> Vec<Trait> {
        let Some(traits) = value.get("traits").and_then(DocNode::as_object) else {
            return Vec::new();
        };
        traits
            .members()
            .map(|(id, member)| Trait {
                id: Name {
                    text: id.to_owned(),
                    span: member.key_span,
                },
                value: node(&member.value),
            })
            .collect()
    }

    fn members(&mut self, node: Option<&DocNode>) -> Vec<Member> {
        let Some(members) = node.and_then(DocNode::as_object) else {
            return Vec::new();
        };
        members
            .members()
            .map(|(name, member)| self.member(name, member.key_span, &member.value))
            .collect()
    }

    /// The fixed members of a list (`member`) or map (`key`, `value`).
    fn named_members(&mut self, value: &DocNode, names: &[&str]) -> Vec<Member> {
        let mut out = Vec::new();
        for name in names {
            if let Some(object) = value.as_object()
                && let Some(member) = object.member(name)
            {
                out.push(self.member(name, member.key_span, &member.value));
            }
        }
        out
    }

    fn member(&mut self, name: &str, key_span: Span, value: &DocNode) -> Member {
        Member {
            name: Name {
                text: name.to_owned(),
                span: key_span,
            },
            target: self.target(Some(value)),
            default: None,
            traits: Self::traits(value),
            doc: None,
            span: value.span,
        }
    }

    /// A service's or resource's properties, as the IDL writes them: lists
    /// and maps of shape ids.
    fn properties(&mut self, value: &DocNode) -> Node {
        let mut members = Vec::new();
        let Some(object) = value.as_object() else {
            return Node {
                value: NodeValue::Object(members),
                span: value.span,
            };
        };
        for (key, member) in object.members() {
            let key_name = Name {
                text: key.to_owned(),
                span: member.key_span,
            };
            let converted = match key {
                "version" => node(&member.value),
                "operations" | "resources" | "errors" | "collectionOperations" => {
                    let items = self
                        .targets(Some(&member.value))
                        .into_iter()
                        .map(shape_id)
                        .collect();
                    Node {
                        value: NodeValue::Array(items),
                        span: member.value.span,
                    }
                }
                "create" | "put" | "read" | "update" | "delete" | "list" => {
                    match self.target(Some(&member.value)) {
                        Some(target) => shape_id(target),
                        None => continue,
                    }
                }
                "identifiers" | "properties" => {
                    let mut entries = Vec::new();
                    for (name, entry) in member
                        .value
                        .as_object()
                        .map(faultline_doc::Object::members)
                        .into_iter()
                        .flatten()
                    {
                        if let Some(target) = self.target(Some(&entry.value)) {
                            entries.push((
                                Name {
                                    text: name.to_owned(),
                                    span: entry.key_span,
                                },
                                shape_id(target),
                            ));
                        }
                    }
                    Node {
                        value: NodeValue::Object(entries),
                        span: member.value.span,
                    }
                }
                _ => continue,
            };
            members.push((key_name, converted));
        }
        Node {
            value: NodeValue::Object(members),
            span: value.span,
        }
    }
}

fn shape_id(name: Name) -> Node {
    Node {
        value: NodeValue::ShapeId(name.text),
        span: name.span,
    }
}

/// A JSON value as a Smithy node value.
fn node(value: &DocNode) -> Node {
    let converted = match &value.value {
        Value::Null => NodeValue::Null,
        Value::Bool(b) => NodeValue::Bool(*b),
        Value::Number(Number::Finite(d)) => NodeValue::Number(d.to_string()),
        Value::Number(other) => NodeValue::String(other.to_string()),
        Value::String(s) => NodeValue::String(s.clone()),
        Value::Array(items) => NodeValue::Array(items.iter().map(node).collect()),
        Value::Object(object) => NodeValue::Object(
            object
                .members()
                .map(|(k, m)| {
                    (
                        Name {
                            text: k.to_owned(),
                            span: m.key_span,
                        },
                        node(&m.value),
                    )
                })
                .collect(),
        ),
    };
    Node {
        value: converted,
        span: value.span,
    }
}

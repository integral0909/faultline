//! The format-independent intermediate representation (IR) of an API
//! contract.
//!
//! Every front end — OpenAPI, JSON Schema, Protobuf, GraphQL, Avro — lowers
//! its input into an [`Api`], and every later stage (comparison, rules,
//! reporting) works on the IR alone. The IR therefore has to express the
//! union of what the formats can say about *values* while staying small enough
//! to reason about:
//!
//! * a [`TypeArena`] of [`TypeNode`]s forming a possibly cyclic graph, whose
//!   children are enumerated in one place ([`graph::for_each_edge`]);
//! * surfaces ([`Operation`]s and [`SchemaRoot`]s) where values enter or leave
//!   the API, exposed uniformly as [`Root`]s;
//! * computed [`Polarity`] for every type, derived from the roots;
//! * stable [`IrPath`] identities for positions;
//! * [`validate`], which checks the invariants the comparison engine relies
//!   on, and [`dump`], a deterministic text rendering.

mod api;
mod arena;
mod dump;
pub mod graph;
mod literal;
pub mod path;
mod polarity;
mod surface;
mod types;
mod validate;

pub use api::{Api, ApiFormat, Root};
pub use arena::{TypeArena, TypeId};
pub use dump::dump;
pub use literal::Literal;
pub use path::{IrPath, PathParseError, Segment};
pub use polarity::{Polarities, Polarity};
pub use surface::{
    Callback, Content, Header, HttpMethod, Operation, OperationKey, ParamLocation, Parameter,
    PathTemplate, RequestBody, Response, Role, SchemaRoot, StatusKey,
};
pub use types::{
    Access, ArrayType, Bound, Contains, Discriminator, EnumType, EnumValue, Field, MapType, Meta,
    NumericType, ObjectType, QualName, StringType, TypeKind, TypeNode, UnionMode, UnionType,
};
pub use validate::{IrError, validate};

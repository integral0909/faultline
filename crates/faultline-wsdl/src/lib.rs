//! The WSDL front end.
//!
//! [`reader`] reads WSDL 1.1 descriptions, with the schemas they embed and
//! the documents they import, into [`model::Definitions`] and XML Schema
//! components, and [`lower_wsdl`] lowers their operations into the IR.

pub mod codes;
mod lower;
pub mod model;
pub mod reader;

pub use lower::{Lowered, lower_wsdl};
pub use reader::{Reader, SOAP_11, SOAP_12, WSDL};

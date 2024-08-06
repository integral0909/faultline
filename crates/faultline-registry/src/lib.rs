//! A schema registry that refuses breaking versions.
//!
//! Teams register each version of an API description under a *subject*;
//! the registry compares it with the versions before it, exactly as
//! `faultline diff` would, and stores it only when no finding fails the
//! subject's settings. Consumers fetch registered versions, whole or one
//! file at a time.
//!
//! [`http`] is a small, bounded HTTP/1.1 server, [`store`] keeps versions on
//! disk, [`compat`] runs the checks, [`service`] maps requests to them, and
//! [`client`] bundles a workspace's files and sends them.

pub mod client;
pub mod compat;
pub mod http;
pub mod service;
pub mod store;

pub use client::{Client, Reply, bundle};
pub use service::Registry;
pub use store::{Bundle, Compatibility, Store};

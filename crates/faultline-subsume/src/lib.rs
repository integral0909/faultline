//! Value-set reasoning for Faultline IR types.
//!
//! A compatibility question is ultimately a question about sets of values:
//! does the new version of a request schema still accept every value the old
//! one accepted? This crate answers such questions between types of two
//! [`Api`](faultline_ir::Api)s:
//!
//! * [`Validator`] decides whether a concrete value belongs to a type;
//! * [`Sampler`] generates values that belong to a type, concentrating on
//!   boundaries where versions tend to differ;
//! * [`Checker`] decides inclusion between types, returning a [`Verdict`] that
//!   is `Yes`, `No` with a verified counterexample, or `Unknown`.
//!
//! The central guarantee: every counterexample has been confirmed by the
//! validator to belong to one type and not the other. A `Yes` is proven by
//! rules that are sound for the constructs involved. Anything else is
//! reported as unknown rather than guessed.

mod check;
pub mod rational;
mod sample;
mod validate;

pub use check::{Checker, Verdict};
pub use sample::Sampler;
pub use validate::{Membership, Reason, Validator};

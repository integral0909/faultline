//! Source text management for Faultline.
//!
//! Every artifact Faultline reports on — a finding, a parse error, a waiver
//! mismatch — must point back at the exact bytes a human wrote. This crate owns
//! that mapping:
//!
//! * [`SourceMap`] stores the decoded text of every file loaded in a session and
//!   hands out compact [`SourceId`]s.
//! * [`Span`] is a half-open byte range inside one source file.
//! * [`LineIndex`] converts byte offsets into line/column positions in the
//!   units different consumers need (bytes, characters, UTF-16 code units).
//! * [`Diagnostic`] is the structured error/warning type shared by all
//!   front ends, with a plain-text renderer in [`render`].
//! * [`Limits`] collects the resource ceilings applied to untrusted input.
//!
//! Offsets are stored as `u32`. Individual source files are therefore limited
//! to 4 GiB, far above the default [`Limits::max_file_bytes`], and in exchange
//! every span fits in 12 bytes.

mod diagnostic;
mod file;
mod limits;
mod line_index;
mod map;
pub mod render;
mod span;

pub use diagnostic::{Diagnostic, DiagnosticCode, Diagnostics, Label, Severity};
pub use file::{DecodeError, SourceFile, SourceName, decode_text};
pub use limits::{LimitExceeded, LimitKind, Limits};
pub use line_index::{LineCol, LineIndex};
pub use map::{SourceError, SourceMap};
pub use span::{SourceId, Span, Spanned};

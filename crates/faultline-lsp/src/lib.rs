//! A language server that shows Faultline's findings while API
//! descriptions are edited.
//!
//! The server compares every API of the workspace (the `[[api]]` entries of
//! `faultline.toml`, or, without any, each open API document) with a base
//! revision, reading open documents from the editor rather than from disk.
//! Findings appear as diagnostics where the change is; hovering shows the
//! rule and an example value, and a quick fix adds a waiver.
//!
//! [`run`] serves the protocol over a reader and a writer (standard input
//! and output for `faultline lsp`); [`Server`] handles one message at a
//! time, which is what tests drive.

mod analysis;
mod overlay;
pub mod protocol;
mod server;
mod uri;

use std::io::{BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use serde_json::Value;

pub use server::{DEFAULT_BASE, Server};

use crate::protocol::{ReadError, code, error_response, read_message, write_message};

/// How long the editor must be quiet before an analysis runs.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

enum Incoming {
    Message(Value),
    /// A message whose body is not JSON.
    Malformed(String),
}

/// Runs an analysis, turning a panic into a message: one malformed input
/// must not take the editor's server down with it.
fn analyze(server: &mut Server) -> Vec<Value> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| server.analyze())).unwrap_or_else(
        |_| {
            vec![protocol::notification(
                "window/showMessage",
                &serde_json::json!({
                    "type": 1,
                    "message": "faultline: internal error while analyzing the workspace; \
                                please report it with the files that caused it",
                }),
            )]
        },
    )
}

/// Serves `server` until the client sends `exit` or closes the stream,
/// returning the process exit code: 0 after an orderly shutdown, 1
/// otherwise.
pub fn run<R, W>(input: R, mut output: W, mut server: Server) -> i32
where
    R: BufRead + Send + 'static,
    W: Write,
{
    let (sender, receiver) = mpsc::channel();
    // Reading blocks, so it gets a thread of its own; the main loop can then
    // wait for either the next message or a quiet moment to analyze in.
    std::thread::spawn(move || {
        let mut input = input;
        loop {
            let incoming = match read_message(&mut input) {
                Ok(Some(message)) => Incoming::Message(message),
                Ok(None) => return,
                Err(ReadError::Json(e)) => Incoming::Malformed(e.to_string()),
                Err(e) => {
                    eprintln!("faultline lsp: {e}");
                    return;
                }
            };
            if sender.send(incoming).is_err() {
                return;
            }
        }
    });
    let mut send = |messages: Vec<Value>| -> bool {
        messages
            .iter()
            .all(|message| write_message(&mut output, message).is_ok())
    };
    loop {
        let next = if server.is_dirty() {
            receiver.recv_timeout(DEBOUNCE)
        } else {
            receiver.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        let replies = match next {
            Ok(Incoming::Message(message)) => server.handle(&message),
            Ok(Incoming::Malformed(reason)) => vec![error_response(
                &Value::Null,
                code::PARSE_ERROR,
                format!("the message is not JSON: {reason}"),
            )],
            Err(RecvTimeoutError::Timeout) => analyze(&mut server),
            Err(RecvTimeoutError::Disconnected) => return 1,
        };
        if !send(replies) {
            return 1;
        }
        if let Some(code) = server.exit_code() {
            return code;
        }
    }
}

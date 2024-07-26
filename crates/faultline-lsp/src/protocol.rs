//! Base protocol: `Content-Length` framed JSON-RPC 2.0 messages.
//!
//! A message is a header block (`Content-Length: N`, optionally
//! `Content-Type`, each line ending in `\r\n`), an empty line, and `N` bytes
//! of UTF-8 JSON.

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

/// The largest message accepted. Editors send whole documents on every
/// change, so this is generous, but a bound keeps a corrupt header from
/// allocating without limit.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// JSON-RPC error codes.
pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    /// The server received a request before `initialize`.
    pub const SERVER_NOT_INITIALIZED: i64 = -32002;
}

/// Why a message could not be read.
#[derive(Debug)]
pub enum ReadError {
    Io(io::Error),
    /// The header block is malformed; the stream cannot be resynchronized.
    Header(String),
    /// The body is not JSON. The framing was intact, so reading may
    /// continue with the next message.
    Json(serde_json::Error),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Io(e) => write!(f, "cannot read a message: {e}"),
            ReadError::Header(message) => write!(f, "malformed message header: {message}"),
            ReadError::Json(e) => write!(f, "message is not JSON: {e}"),
        }
    }
}

impl std::error::Error for ReadError {}

/// Reads the next message, or `None` at the end of the stream.
pub fn read_message(reader: &mut impl BufRead) -> Result<Option<Value>, ReadError> {
    let mut length = None;
    let mut line = String::new();
    let mut first = true;
    loop {
        line.clear();
        let n = reader.read_line(&mut line).map_err(ReadError::Io)?;
        if n == 0 {
            return if first {
                Ok(None)
            } else {
                Err(ReadError::Header("the stream ended inside a header".into()))
            };
        }
        first = false;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        let Some((name, value)) = trimmed.split_once(':') else {
            return Err(ReadError::Header(format!(
                "expected `Name: value`, got {trimmed:?}"
            )));
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let n: usize = value
                .trim()
                .parse()
                .map_err(|_| ReadError::Header(format!("invalid Content-Length {value:?}")))?;
            if n > MAX_MESSAGE_BYTES {
                return Err(ReadError::Header(format!(
                    "a message of {n} bytes exceeds the limit of {MAX_MESSAGE_BYTES}"
                )));
            }
            length = Some(n);
        }
    }
    let length = length.ok_or_else(|| ReadError::Header("no Content-Length".into()))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(ReadError::Io)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(ReadError::Json)
}

/// Writes one message and flushes it.
pub fn write_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_string(message).expect("JSON values always serialize");
    write!(writer, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    writer.flush()
}

/// A successful response to the request `id`.
pub fn response(id: &Value, result: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// An error response to the request `id`.
pub fn error_response(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message.into()},
    })
}

/// A notification from the server.
pub fn notification(method: &str, params: &Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(body: &str) -> String {
        format!("Content-Length: {}\r\n\r\n{body}", body.len())
    }

    #[test]
    fn reads_consecutive_messages() {
        let text = format!(
            "{}{}",
            frame(r#"{"a":1}"#),
            frame(r#"{"b":"é"}"#).replace("Content-Length", "content-length")
        );
        let mut reader = text.as_bytes();
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({"a": 1})));
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({"b": "é"})));
        assert!(read_message(&mut reader).unwrap().is_none());
    }

    #[test]
    fn content_type_is_ignored() {
        let text = "Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: 2\r\n\r\n{}";
        let mut reader = text.as_bytes();
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({})));
    }

    #[test]
    fn malformed_headers() {
        for text in [
            "Content-Length: x\r\n\r\n{}",
            "\r\n{}",
            "garbage\r\n\r\n",
            "Content-Length: 99999999999\r\n\r\n",
            "Content-Length: 2\r\n",
        ] {
            let mut reader = text.as_bytes();
            assert!(
                matches!(read_message(&mut reader), Err(ReadError::Header(_))),
                "{text:?}"
            );
        }
        let mut truncated = "Content-Length: 10\r\n\r\n{}".as_bytes();
        assert!(matches!(
            read_message(&mut truncated),
            Err(ReadError::Io(_))
        ));
        let not_json = frame("{x");
        let mut reader = not_json.as_bytes();
        assert!(matches!(read_message(&mut reader), Err(ReadError::Json(_))));
    }

    #[test]
    fn writes_framed_messages() {
        let mut out = Vec::new();
        write_message(&mut out, &json!({"x": "ü"})).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "Content-Length: 10\r\n\r\n{\"x\":\"ü\"}");
        let mut reader = text.as_bytes();
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({"x": "ü"})));
    }
}

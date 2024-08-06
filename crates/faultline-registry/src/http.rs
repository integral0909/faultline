//! A small HTTP/1.1 server: one request per connection, bodies framed by
//! `Content-Length` only, and every size and wait bounded.
//!
//! The registry's clients are CI jobs and tools that send one request and
//! read one response, so connection reuse and chunked uploads buy little,
//! and leaving them out removes the framing ambiguities request smuggling
//! relies on. Requests with `Transfer-Encoding` are refused.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use faultline_doc::uri::percent_decode;

/// Limits and resources of a server.
#[derive(Clone, Debug)]
pub struct Options {
    /// Worker threads, each serving one connection at a time.
    pub threads: usize,
    /// The largest request body accepted.
    pub max_body_bytes: usize,
    /// The largest request line and header block accepted.
    pub max_header_bytes: usize,
    /// How long a client may take to send its request, or to read the
    /// response.
    pub timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            threads: 8,
            max_body_bytes: 16 * 1024 * 1024,
            max_header_bytes: 16 * 1024,
            timeout: Duration::from_secs(30),
        }
    }
}

/// A parsed request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// Decoded path segments: `/subjects/pets%2Fv1` is `["subjects",
    /// "pets/v1"]`.
    pub segments: Vec<String>,
    /// Decoded query parameters; a repeated name keeps its last value.
    pub query: BTreeMap<String, String>,
    /// Headers by lower-case name; repeated headers are joined with `, `.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// A response to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        let mut body = serde_json::to_vec_pretty(value).expect("JSON values always serialize");
        body.push(b'\n');
        Self {
            status,
            content_type: "application/json",
            headers: Vec::new(),
            body,
        }
    }

    pub fn text(status: u16, text: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            headers: Vec::new(),
            body: text.into().into_bytes(),
        }
    }

    /// A JSON error document: `{"error": message}`.
    pub fn error(status: u16, message: impl Into<String>) -> Self {
        Self::json(status, &serde_json::json!({"error": message.into()}))
    }

    #[must_use]
    pub fn with_header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    fn write_to(&self, out: &mut impl Write) -> io::Result<()> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\
             Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n",
            self.status,
            reason(self.status),
            self.content_type,
            self.body.len()
        );
        for (name, value) in &self.headers {
            // Values come from the server itself, but a line break would
            // still split the header block.
            let value: String = value.chars().filter(|c| !c.is_control()).collect();
            let _ = write!(head, "{name}: {value}\r\n");
        }
        head.push_str("\r\n");
        out.write_all(head.as_bytes())?;
        out.write_all(&self.body)?;
        out.flush()
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Content",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "Unknown",
    }
}

/// Why a request could not be read; each maps to the response sent.
#[derive(Debug, PartialEq, Eq)]
pub enum RequestError {
    /// The connection closed or timed out before a request arrived: no
    /// response is sent.
    Closed,
    Malformed(String),
    HeadersTooLarge,
    BodyTooLarge {
        limit: usize,
    },
    LengthRequired,
    Unsupported(String),
    Version,
    Timeout,
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequestError::Closed => f.write_str("the connection closed"),
            RequestError::Malformed(reason) => write!(f, "malformed request: {reason}"),
            RequestError::HeadersTooLarge => f.write_str("the request headers are too large"),
            RequestError::BodyTooLarge { limit } => {
                write!(f, "the request body exceeds the limit of {limit} bytes")
            }
            RequestError::LengthRequired => f.write_str("a request body needs a Content-Length"),
            RequestError::Unsupported(what) => write!(f, "{what} is not supported"),
            RequestError::Version => f.write_str("only HTTP/1.0 and HTTP/1.1 are supported"),
            RequestError::Timeout => f.write_str("the request was not received in time"),
        }
    }
}

impl RequestError {
    fn response(&self) -> Option<Response> {
        let status = match self {
            RequestError::Closed => return None,
            RequestError::Malformed(_) => 400,
            RequestError::HeadersTooLarge => 431,
            RequestError::BodyTooLarge { .. } => 413,
            RequestError::LengthRequired => 411,
            RequestError::Unsupported(_) => 501,
            RequestError::Version => 505,
            RequestError::Timeout => 408,
        };
        Some(Response::error(status, self.to_string()))
    }
}

fn io_error(e: &io::Error) -> RequestError {
    match e.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => RequestError::Timeout,
        _ => RequestError::Closed,
    }
}

/// Reads one request. `continue_to` receives `100 Continue` when the client
/// asks for it before sending its body.
pub fn read_request(
    reader: &mut impl BufRead,
    continue_to: &mut impl Write,
    options: &Options,
) -> Result<Request, RequestError> {
    let mut budget = options.max_header_bytes;
    let Some(request_line) = read_line(reader, &mut budget)? else {
        return Err(RequestError::Closed);
    };
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(RequestError::Malformed("invalid request line".into()));
    };
    if !method.bytes().all(|b| b.is_ascii_uppercase()) || method.is_empty() {
        return Err(RequestError::Malformed("invalid method".into()));
    }
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(RequestError::Version);
    }
    let (segments, query) = parse_target(target)?;

    let (headers, lengths) = read_headers(reader, &mut budget)?;
    if headers.contains_key("transfer-encoding") {
        return Err(RequestError::Unsupported("Transfer-Encoding".into()));
    }
    let length = match lengths.as_slice() {
        [] => None,
        [first, rest @ ..] if rest.iter().all(|l| l == first) => Some(
            first
                .parse::<usize>()
                .ok()
                .filter(|_| first.bytes().all(|b| b.is_ascii_digit()))
                .ok_or_else(|| RequestError::Malformed("invalid Content-Length".into()))?,
        ),
        _ => {
            return Err(RequestError::Malformed(
                "conflicting Content-Length headers".into(),
            ));
        }
    };
    let body = match length {
        None if matches!(method, "POST" | "PUT" | "PATCH") => {
            return Err(RequestError::LengthRequired);
        }
        None | Some(0) => Vec::new(),
        Some(n) if n > options.max_body_bytes => {
            return Err(RequestError::BodyTooLarge {
                limit: options.max_body_bytes,
            });
        }
        Some(n) => {
            if headers
                .get("expect")
                .is_some_and(|e| e.eq_ignore_ascii_case("100-continue"))
            {
                continue_to
                    .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                    .and_then(|()| continue_to.flush())
                    .map_err(|e| io_error(&e))?;
            }
            let mut body = vec![0; n];
            reader.read_exact(&mut body).map_err(|e| match e.kind() {
                io::ErrorKind::UnexpectedEof => {
                    RequestError::Malformed("the body is shorter than its Content-Length".into())
                }
                _ => io_error(&e),
            })?;
            body
        }
    };
    Ok(Request {
        method: method.to_owned(),
        segments,
        query,
        headers,
        body,
    })
}

type Headers = (BTreeMap<String, String>, Vec<String>);

/// The header block: headers by lower-case name, and every
/// `Content-Length` value as sent.
fn read_headers(reader: &mut impl BufRead, budget: &mut usize) -> Result<Headers, RequestError> {
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    let mut lengths = Vec::new();
    loop {
        let Some(line) = read_line(reader, budget)? else {
            return Err(RequestError::Malformed("the headers are incomplete".into()));
        };
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(RequestError::Malformed(format!(
                "invalid header line {line:?}"
            )));
        };
        if name.is_empty() || name.bytes().any(|b| !is_token(b)) {
            return Err(RequestError::Malformed("invalid header name".into()));
        }
        let name = name.to_ascii_lowercase();
        let value = value.trim_matches([' ', '\t']).to_owned();
        if name == "content-length" {
            lengths.push(value.clone());
        }
        headers
            .entry(name)
            .and_modify(|existing| {
                existing.push_str(", ");
                existing.push_str(&value);
            })
            .or_insert(value);
    }
    Ok((headers, lengths))
}

fn is_token(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// One CRLF- (or LF-) terminated line within the header budget; `None` at
/// the end of the stream before any byte.
fn read_line(
    reader: &mut impl BufRead,
    budget: &mut usize,
) -> Result<Option<String>, RequestError> {
    let mut line = Vec::new();
    loop {
        let available = match reader.fill_buf() {
            Ok(buf) => buf,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_error(&e)),
        };
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(RequestError::Malformed("the request ended mid-line".into()))
            };
        }
        let (chunk, done) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (&available[..=i], true),
            None => (available, false),
        };
        if chunk.len() > *budget {
            return Err(RequestError::HeadersTooLarge);
        }
        *budget -= chunk.len();
        line.extend_from_slice(chunk);
        let used = chunk.len();
        reader.consume(used);
        if done {
            break;
        }
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|_| RequestError::Malformed("the request line or a header is not UTF-8".into()))
}

type Target = (Vec<String>, BTreeMap<String, String>);

/// Splits an origin-form target into decoded path segments and query
/// parameters.
fn parse_target(target: &str) -> Result<Target, RequestError> {
    let malformed = |what: &str| RequestError::Malformed(what.to_owned());
    if !target.starts_with('/') {
        return Err(malformed("the request target must be a path"));
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut segments = Vec::new();
    for raw in path.split('/').skip(1) {
        if raw.is_empty() {
            continue;
        }
        let decoded = percent_decode(raw).ok_or_else(|| malformed("invalid percent-encoding"))?;
        if decoded.chars().any(char::is_control) {
            return Err(malformed("control characters in the path"));
        }
        segments.push(decoded);
    }
    let mut parameters = BTreeMap::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let decode = |text: &str| {
            percent_decode(&text.replace('+', " "))
                .ok_or_else(|| malformed("invalid percent-encoding in the query"))
        };
        parameters.insert(decode(name)?, decode(value)?);
    }
    Ok((segments, parameters))
}

/// A request handler.
pub type Handler = dyn Fn(&Request) -> Response + Send + Sync;

/// A running server.
pub struct Server {
    address: SocketAddr,
    stopping: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
}

impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

impl Server {
    /// Listens on `address` and serves requests with `handler` on
    /// `options.threads` threads.
    pub fn start(
        address: impl ToSocketAddrs,
        options: Options,
        handler: &Arc<Handler>,
    ) -> io::Result<Server> {
        let listener = TcpListener::bind(address)?;
        let address = listener.local_addr()?;
        let stopping = Arc::new(AtomicBool::new(false));
        let options = Arc::new(options);
        let mut workers = Vec::new();
        for _ in 0..options.threads.max(1) {
            let listener = listener.try_clone()?;
            let (stopping, options, handler) = (
                Arc::clone(&stopping),
                Arc::clone(&options),
                Arc::clone(handler),
            );
            workers.push(std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stopping.load(Ordering::SeqCst) {
                        return;
                    }
                    if let Ok(stream) = stream {
                        serve_connection(stream, &options, handler.as_ref());
                    }
                }
            }));
        }
        Ok(Server {
            address,
            stopping,
            workers,
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Serves until the process ends.
    pub fn wait(self) {
        for worker in self.workers {
            let _ = worker.join();
        }
    }

    /// Stops accepting connections and waits for requests in progress.
    pub fn stop(self) {
        self.stopping.store(true, Ordering::SeqCst);
        // Each blocked `accept` needs a connection to notice.
        for _ in &self.workers {
            let _ = TcpStream::connect_timeout(&self.address, Duration::from_secs(1));
        }
        self.wait();
    }
}

fn serve_connection(stream: TcpStream, options: &Options, handler: &Handler) {
    let _ = stream.set_read_timeout(Some(options.timeout));
    let _ = stream.set_write_timeout(Some(options.timeout));
    let _ = stream.set_nodelay(true);
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    let response = match read_request(&mut reader, &mut writer, options) {
        Ok(request) => {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(&request)));
            result.unwrap_or_else(|_| Response::error(500, "internal error"))
        }
        Err(e) => match e.response() {
            Some(response) => response,
            None => return,
        },
    };
    let _ = response.write_to(&mut writer);
    let _ = writer.shutdown(std::net::Shutdown::Write);
    // Drain what the client may still be sending (a refused body), briefly,
    // so closing does not reset the connection before it reads the reply.
    let _ = reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(200)));
    let _ = io::copy(&mut reader.take(1024 * 1024), &mut io::sink());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Request, RequestError> {
        parse_with(text, &Options::default())
    }

    fn parse_with(text: &str, options: &Options) -> Result<Request, RequestError> {
        let mut reader = text.as_bytes();
        read_request(&mut reader, &mut io::sink(), options)
    }

    #[test]
    fn requests() {
        let request = parse(
            "POST /subjects/pets%2Fv1/versions?dry+run=yes&x HTTP/1.1\r\nHost: a\r\n\
             Content-Length: 4\r\nX-A: 1\r\nx-a: 2\r\n\r\nbody",
        )
        .unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.segments, ["subjects", "pets/v1", "versions"]);
        assert_eq!(request.query["dry run"], "yes");
        assert_eq!(request.query["x"], "");
        assert_eq!(request.header("x-a"), Some("1, 2"));
        assert_eq!(request.body, b"body");
        let get = parse("GET / HTTP/1.0\n\n").unwrap();
        assert!(get.segments.is_empty() && get.body.is_empty());
    }

    #[test]
    fn refusals() {
        let status = |text: &str| {
            parse(text)
                .err()
                .and_then(|e| e.response())
                .map(|r| r.status)
        };
        assert_eq!(status(""), None, "nothing sent, nothing answered");
        assert_eq!(status("GET /\r\n\r\n"), Some(400));
        assert_eq!(status("get / HTTP/1.1\r\n\r\n"), Some(400));
        assert_eq!(status("GET / HTTP/2\r\n\r\n"), Some(505));
        assert_eq!(status("GET http://a/ HTTP/1.1\r\n\r\n"), Some(400));
        assert_eq!(status("GET /%zz HTTP/1.1\r\n\r\n"), Some(400));
        assert_eq!(status("GET /a%0Ab HTTP/1.1\r\n\r\n"), Some(400));
        assert_eq!(status("GET / HTTP/1.1\r\nBad Header: x\r\n\r\n"), Some(400));
        assert_eq!(status("GET / HTTP/1.1\r\nNoColon\r\n\r\n"), Some(400));
        assert_eq!(status("GET / HTTP/1.1\r\nHost: a\r\n"), Some(400));
        assert_eq!(status("POST / HTTP/1.1\r\n\r\n"), Some(411));
        assert_eq!(
            status("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"),
            Some(501)
        );
        assert_eq!(
            status("POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab"),
            Some(400)
        );
        assert_eq!(
            status("POST / HTTP/1.1\r\nContent-Length: +1\r\n\r\na"),
            Some(400)
        );
        assert_eq!(
            status("POST / HTTP/1.1\r\nContent-Length: 5\r\n\r\nab"),
            Some(400)
        );
        let small = Options {
            max_body_bytes: 3,
            max_header_bytes: 40,
            ..Options::default()
        };
        let code = |text: &str| {
            parse_with(text, &small)
                .unwrap_err()
                .response()
                .unwrap()
                .status
        };
        assert_eq!(
            code("POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\nabcd"),
            413
        );
        assert_eq!(
            code(&format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(64))),
            431
        );
    }

    #[test]
    fn expect_continue() {
        let mut reader =
            "PUT /x HTTP/1.1\r\nExpect: 100-continue\r\nContent-Length: 2\r\n\r\nok".as_bytes();
        let mut interim = Vec::new();
        let request = read_request(&mut reader, &mut interim, &Options::default()).unwrap();
        assert_eq!(request.body, b"ok");
        assert_eq!(interim, b"HTTP/1.1 100 Continue\r\n\r\n");
    }

    #[test]
    fn responses() {
        let mut out = Vec::new();
        Response::text(404, "no")
            .with_header("Location", "/a\r\nInjected: 1")
            .write_to(&mut out)
            .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(text.contains("Content-Length: 2\r\n"));
        assert!(text.contains("Location: /aInjected: 1\r\n"));
        assert!(text.ends_with("\r\n\r\nno"));
    }

    #[test]
    fn serves_over_tcp() {
        let handler: Arc<Handler> = Arc::new(|request: &Request| {
            assert!(request.segments != ["panic"], "handler failure");
            Response::text(200, request.segments.join(","))
        });
        let server = Server::start("127.0.0.1:0", Options::default(), &handler).unwrap();
        let exchange = |request: &str| {
            let mut stream = TcpStream::connect(server.address()).unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        };
        let ok = exchange("GET /a/b HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(ok.starts_with("HTTP/1.1 200 OK"), "{ok}");
        assert!(ok.ends_with("a,b"), "{ok}");
        let failed = exchange("GET /panic HTTP/1.1\r\n\r\n");
        assert!(failed.starts_with("HTTP/1.1 500"), "{failed}");
        let bad = exchange("BAD\r\n\r\n");
        assert!(bad.starts_with("HTTP/1.1 400"), "{bad}");
        server.stop();
    }
}

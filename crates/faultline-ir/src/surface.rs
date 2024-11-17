//! Surfaces: the entry points through which values cross the API boundary.
//!
//! Types only acquire a direction (request vs. response) by being reachable
//! from a surface, which is why polarity is computed from here.

use std::fmt;

use faultline_source::Span;

use crate::arena::TypeId;
use crate::types::Meta;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HttpMethod {
    Get,
    Put,
    Post,
    Delete,
    Options,
    Head,
    Patch,
    Trace,
}

impl HttpMethod {
    pub const ALL: [HttpMethod; 8] = [
        HttpMethod::Get,
        HttpMethod::Put,
        HttpMethod::Post,
        HttpMethod::Delete,
        HttpMethod::Options,
        HttpMethod::Head,
        HttpMethod::Patch,
        HttpMethod::Trace,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Put => "PUT",
            HttpMethod::Post => "POST",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Options => "OPTIONS",
            HttpMethod::Head => "HEAD",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Trace => "TRACE",
        }
    }

    /// Parses a method name case-insensitively (OpenAPI uses lowercase keys).
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|m| m.as_str().eq_ignore_ascii_case(name))
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A URL path template such as `/pets/{petId}`.
///
/// Two templates that differ only in parameter *names* describe the same
/// endpoint, so identity uses the [normalized](Self::normalized) form with every
/// parameter written as `{}`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PathTemplate {
    raw: String,
    normalized: String,
    params: Vec<String>,
}

impl PathTemplate {
    pub fn parse(raw: &str) -> Self {
        let mut normalized = String::with_capacity(raw.len());
        let mut params = Vec::new();
        let mut rest = raw;
        while let Some(open) = rest.find('{') {
            let Some(close) = rest[open..].find('}') else {
                break;
            };
            normalized.push_str(&rest[..open]);
            normalized.push_str("{}");
            params.push(rest[open + 1..open + close].to_owned());
            rest = &rest[open + close + 1..];
        }
        normalized.push_str(rest);
        Self {
            raw: raw.to_owned(),
            normalized,
            params,
        }
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn normalized(&self) -> &str {
        &self.normalized
    }

    /// Parameter names in the order they appear.
    pub fn params(&self) -> &[String] {
        &self.params
    }
}

/// What identifies an operation across versions.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum OperationKey {
    Http {
        method: HttpMethod,
        path: PathTemplate,
    },
    /// A remote procedure: `service` is fully qualified
    /// (`petstore.v1.PetService`), as it appears in the gRPC method path.
    Rpc { service: String, method: String },
    /// Messages the application sends or receives on a channel (AsyncAPI).
    /// `channel` is the channel's address, a template like
    /// `orders/{region}/created`.
    Message {
        action: MessageAction,
        channel: PathTemplate,
    },
}

/// Which way an application's messages flow, from its own point of view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MessageAction {
    /// The application publishes messages; consumers receive them.
    Send,
    /// The application consumes messages that others publish.
    Receive,
}

impl MessageAction {
    pub fn as_str(self) -> &'static str {
        match self {
            MessageAction::Send => "send",
            MessageAction::Receive => "receive",
        }
    }
}

impl fmt::Display for MessageAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl OperationKey {
    /// The identity used for matching and in [`IrPath`](crate::IrPath)s,
    /// e.g. `GET /pets/{}`.
    pub fn identity(&self) -> String {
        match self {
            OperationKey::Http { method, path } => format!("{method} {}", path.normalized()),
            OperationKey::Rpc { service, method } => format!("{service}/{method}"),
            OperationKey::Message { action, channel } => {
                format!("{action} {}", channel.normalized())
            }
        }
    }

    /// Whether the operation carries messages rather than requests.
    pub fn is_message(&self) -> bool {
        matches!(self, OperationKey::Message { .. })
    }
}

impl fmt::Display for OperationKey {
    /// The human-readable form, keeping parameter names: `GET /pets/{petId}`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OperationKey::Http { method, path } => write!(f, "{method} {}", path.raw()),
            OperationKey::Rpc { service, method } => write!(f, "{service}/{method}"),
            OperationKey::Message { action, channel } => write!(f, "{action} {}", channel.raw()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParamLocation {
    Path,
    Query,
    Header,
    Cookie,
}

impl ParamLocation {
    pub fn as_str(self) -> &'static str {
        match self {
            ParamLocation::Path => "path",
            ParamLocation::Query => "query",
            ParamLocation::Header => "header",
            ParamLocation::Cookie => "cookie",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "path" => Some(ParamLocation::Path),
            "query" => Some(ParamLocation::Query),
            "header" => Some(ParamLocation::Header),
            "cookie" => Some(ParamLocation::Cookie),
            _ => None,
        }
    }
}

impl fmt::Display for ParamLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Parameter {
    pub location: ParamLocation,
    /// For header parameters, the name as written; header identity is
    /// case-insensitive (see [`identity_name`](Self::identity_name)).
    pub name: String,
    pub required: bool,
    pub ty: TypeId,
    /// How the value is written into the request, where the format says.
    pub serialization: Option<Serialization>,
    pub meta: Meta,
    pub origin: Option<Span>,
}

/// How a parameter value is encoded on the wire. Front ends store the
/// *effective* values, with the format's defaults applied, so that spelling a
/// default explicitly is not a change.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Serialization {
    /// `form`, `simple`, `matrix`, `label`, `spaceDelimited`, `pipeDelimited`
    /// or `deepObject` in OpenAPI.
    pub style: String,
    pub explode: bool,
    /// Reserved characters may appear unencoded.
    pub allow_reserved: bool,
    /// An empty value may be sent.
    pub allow_empty_value: bool,
}

/// One acceptable combination of security schemes: all of them are needed.
/// An operation's requirements are alternatives; any one suffices.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecurityRequirement {
    /// Scheme name to required scopes (sorted).
    pub schemes: std::collections::BTreeMap<String, Vec<String>>,
}

impl SecurityRequirement {
    /// Whether this requirement needs no credentials at all.
    pub fn is_anonymous(&self) -> bool {
        self.schemes.is_empty()
    }
}

impl fmt::Display for SecurityRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.schemes.is_empty() {
            return f.write_str("{}");
        }
        let parts: Vec<String> = self
            .schemes
            .iter()
            .map(|(name, scopes)| {
                if scopes.is_empty() {
                    name.clone()
                } else {
                    format!("{name}[{}]", scopes.join(","))
                }
            })
            .collect();
        f.write_str(&parts.join(" + "))
    }
}

impl Parameter {
    /// The name used to match this parameter across versions. HTTP header
    /// names are case-insensitive (RFC 9110 §5.1).
    pub fn identity_name(&self) -> String {
        match self.location {
            ParamLocation::Header => self.name.to_ascii_lowercase(),
            _ => self.name.clone(),
        }
    }
}

/// A payload for one media type.
#[derive(Clone, Debug, PartialEq)]
pub struct Content {
    pub media_type: String,
    pub ty: TypeId,
    pub origin: Option<Span>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RequestBody {
    pub required: bool,
    pub content: Vec<Content>,
    pub meta: Meta,
    pub origin: Option<Span>,
}

/// Which responses a [`Response`] entry covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StatusKey {
    Code(u16),
    /// `1XX`–`5XX`, holding the leading digit.
    Range(u8),
    Default,
}

impl StatusKey {
    /// Parses `200`, `4XX` (case-insensitive) or `default`.
    pub fn parse(text: &str) -> Option<Self> {
        if text == "default" {
            return Some(StatusKey::Default);
        }
        let bytes = text.as_bytes();
        match bytes {
            [d @ b'1'..=b'5', x, y]
                if x.eq_ignore_ascii_case(&b'x') && y.eq_ignore_ascii_case(&b'x') =>
            {
                Some(StatusKey::Range(d - b'0'))
            }
            [b'1'..=b'5', b'0'..=b'9', b'0'..=b'9'] => text.parse().ok().map(StatusKey::Code),
            _ => None,
        }
    }
}

impl fmt::Display for StatusKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatusKey::Code(code) => write!(f, "{code}"),
            StatusKey::Range(digit) => write!(f, "{digit}XX"),
            StatusKey::Default => f.write_str("default"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub name: String,
    pub required: bool,
    pub ty: TypeId,
    pub meta: Meta,
    pub origin: Option<Span>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub status: StatusKey,
    pub headers: Vec<Header>,
    pub content: Vec<Content>,
    pub meta: Meta,
    pub origin: Option<Span>,
}

/// Requests the API makes back to the client (OpenAPI callbacks). Values
/// flow the opposite way from the enclosing operation.
#[derive(Clone, Debug, PartialEq)]
pub struct Callback {
    pub name: String,
    pub operations: Vec<Operation>,
    pub origin: Option<Span>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Operation {
    pub key: OperationKey,
    pub operation_id: Option<String>,
    pub parameters: Vec<Parameter>,
    pub request_body: Option<RequestBody>,
    pub responses: Vec<Response>,
    pub callbacks: Vec<Callback>,
    /// Alternative security requirements (any one suffices). `None` when the
    /// format has no notion of security; an empty list means none apply.
    pub security: Option<Vec<SecurityRequirement>>,
    /// For RPC operations: which directions carry a stream of messages
    /// rather than one.
    pub streaming: Option<Streaming>,
    /// For RPC operations: the caller sends the request and expects no
    /// reply at all (Thrift `oneway`, WSDL one-way operations).
    pub one_way: bool,
    pub meta: Meta,
    pub origin: Option<Span>,
}

/// Whether each direction of an RPC is a stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Streaming {
    pub request: bool,
    pub response: bool,
}

impl Operation {
    pub fn new(key: OperationKey) -> Self {
        Self {
            key,
            operation_id: None,
            parameters: Vec::new(),
            request_body: None,
            responses: Vec::new(),
            callbacks: Vec::new(),
            security: None,
            streaming: None,
            one_way: false,
            meta: Meta::default(),
            origin: None,
        }
    }
}

/// The direction a standalone schema is used in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// Documents are written by others and read by the schema owner (for
    /// example, a configuration file format).
    Input,
    /// Documents are produced by the schema owner (for example, an event
    /// payload published to consumers).
    Output,
    /// Both directions; changes must preserve the exact value set.
    Both,
}

/// A schema that is itself the contract, without operations around it: a
/// JSON Schema document, an Avro record.
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaRoot {
    pub name: String,
    pub ty: TypeId,
    pub role: Role,
    pub origin: Option<Span>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_templates_normalize_parameter_names() {
        let a = PathTemplate::parse("/pets/{petId}/toys/{toy_id}");
        let b = PathTemplate::parse("/pets/{id}/toys/{t}");
        assert_eq!(a.normalized(), "/pets/{}/toys/{}");
        assert_eq!(a.normalized(), b.normalized());
        assert_eq!(a.params(), ["petId", "toy_id"]);
        assert_eq!(
            PathTemplate::parse("/a/{unclosed").normalized(),
            "/a/{unclosed"
        );
        assert_eq!(
            PathTemplate::parse("/files/{name}.{ext}").normalized(),
            "/files/{}.{}"
        );
    }

    #[test]
    fn operation_identity_ignores_parameter_names_but_display_keeps_them() {
        let key = OperationKey::Http {
            method: HttpMethod::parse("get").unwrap(),
            path: PathTemplate::parse("/pets/{petId}"),
        };
        assert_eq!(key.identity(), "GET /pets/{}");
        assert_eq!(key.to_string(), "GET /pets/{petId}");
    }

    #[test]
    fn status_keys() {
        assert_eq!(StatusKey::parse("200"), Some(StatusKey::Code(200)));
        assert_eq!(StatusKey::parse("4xx"), Some(StatusKey::Range(4)));
        assert_eq!(StatusKey::parse("5XX").unwrap().to_string(), "5XX");
        assert_eq!(StatusKey::parse("default"), Some(StatusKey::Default));
        for bad in ["600", "20", "2000", "6XX", "Default", "abc", ""] {
            assert_eq!(StatusKey::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn header_parameters_match_case_insensitively() {
        let p = Parameter {
            location: ParamLocation::Header,
            name: "X-Request-ID".into(),
            required: false,
            ty: TypeId::from_raw(0),
            serialization: None,
            meta: Meta::default(),
            origin: None,
        };
        assert_eq!(p.identity_name(), "x-request-id");
    }
}

//! The reader: XML 1.0 with namespaces, spans on everything, and no DTDs.
//!
//! Document type declarations are refused rather than processed. Nothing
//! that describes an API needs them, and processing them is where XML's
//! dangers live: entity expansion ("billion laughs") and external entities
//! that read local files or make network requests. Without a DTD the only
//! entities are the five predefined ones and character references.
//!
//! Elements are read with an explicit stack instead of recursion, so a
//! deeply nested document cannot overflow the thread's stack; depth and
//! the number of nodes are still bounded by [`Limits`].

use faultline_source::{Diagnostic, Diagnostics, Limits, SourceId, Span};

use crate::codes;
use crate::tree::{Attribute, Content, Document, Element, QName};

/// The namespace of the `xmlns` attributes themselves.
pub const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";
/// The namespace the `xml` prefix is bound to.
pub const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

struct Fatal;

type PResult<T> = Result<T, Fatal>;

/// Reads `text` as an XML document. The first well-formedness error is
/// fatal, as XML requires; it is reported in `diagnostics`.
pub fn parse(
    text: &str,
    source: SourceId,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Option<Document> {
    let mut parser = Parser {
        text,
        bytes: text.as_bytes(),
        pos: 0,
        source,
        max_depth: limits.max_depth.min(crate::HARD_MAX_DEPTH),
        max_nodes: limits.max_document_nodes,
        nodes: 0,
        diagnostics,
    };
    parser.document().ok()
}

/// A start tag whose element is still open.
struct Open {
    raw_name: String,
    prefix: Option<String>,
    local: String,
    raw_attributes: Vec<RawAttribute>,
    children: Vec<Content>,
    namespaces: Vec<(String, String)>,
    start: usize,
    name_span: Span,
}

struct RawAttribute {
    raw_name: String,
    value: String,
    span: Span,
    value_span: Span,
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    source: SourceId,
    max_depth: u32,
    max_nodes: u64,
    nodes: u64,
    diagnostics: &'a mut Diagnostics,
}

impl Parser<'_> {
    fn span(&self, start: usize, end: usize) -> Span {
        Span::new(
            self.source,
            u32::try_from(start).unwrap_or(u32::MAX),
            u32::try_from(end).unwrap_or(u32::MAX),
        )
    }

    fn fail<T>(
        &mut self,
        code: faultline_source::DiagnosticCode,
        message: impl Into<String>,
        start: usize,
        end: usize,
    ) -> PResult<T> {
        let span = self.span(start, end.max(start));
        self.diagnostics.push(
            Diagnostic::error(message.into())
                .with_code(code)
                .with_primary(span, ""),
        );
        Err(Fatal)
    }

    fn unexpected<T>(&mut self, expected: &str) -> PResult<T> {
        if self.at_end() {
            let end = self.bytes.len();
            return self.fail(
                codes::SYNTAX,
                format!("the document ended where {expected} was expected"),
                end,
                end,
            );
        }
        let c = self.text[self.pos..].chars().next().unwrap_or('\0');
        let end = self.pos + c.len_utf8();
        self.fail(
            codes::SYNTAX,
            format!("expected {expected}, found {c:?}"),
            self.pos,
            end,
        )
    }

    fn at_end(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn starts_with(&self, s: &str) -> bool {
        self.bytes[self.pos..].starts_with(s.as_bytes())
    }

    fn skip_whitespace(&mut self) -> bool {
        let start = self.pos;
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
        self.pos > start
    }

    fn expect(&mut self, s: &str) -> PResult<()> {
        if self.starts_with(s) {
            self.pos += s.len();
            Ok(())
        } else {
            self.unexpected(&format!("`{s}`"))
        }
    }

    fn count_node(&mut self, start: usize) -> PResult<()> {
        self.nodes += 1;
        if self.nodes > self.max_nodes {
            let message = format!(
                "the document has more than {} nodes, the limit",
                self.max_nodes
            );
            return self.fail(codes::NODE_LIMIT, message, start, self.pos);
        }
        Ok(())
    }

    fn document(&mut self) -> PResult<Document> {
        if self.text.starts_with('\u{feff}') {
            self.pos = 3;
        }
        if self.starts_with("<?xml")
            && matches!(
                self.bytes.get(self.pos + 5),
                Some(b' ' | b'\t' | b'\n' | b'\r' | b'?')
            )
        {
            self.processing_instruction()?;
        }
        self.misc()?;
        if self.starts_with("<!DOCTYPE") {
            let start = self.pos;
            return self.fail(
                codes::DTD,
                "document type declarations are not supported: they are not needed to describe an API, and their entities are unsafe to expand",
                start,
                start + "<!DOCTYPE".len(),
            );
        }
        if self.peek() != Some(b'<') {
            return self.unexpected("the root element");
        }
        let root = self.element()?;
        self.misc()?;
        if !self.at_end() {
            let start = self.pos;
            return self.fail(
                codes::SYNTAX,
                "content after the root element",
                start,
                self.bytes.len(),
            );
        }
        Ok(Document { root })
    }

    /// Whitespace, comments and processing instructions outside the root.
    fn misc(&mut self) -> PResult<()> {
        loop {
            self.skip_whitespace();
            if self.starts_with("<!--") {
                self.comment()?;
            } else if self.starts_with("<?") {
                self.processing_instruction()?;
            } else {
                return Ok(());
            }
        }
    }

    fn comment(&mut self) -> PResult<()> {
        let start = self.pos;
        self.pos += 4;
        match self.text[self.pos..].find("-->") {
            Some(end) => {
                self.pos += end + 3;
                Ok(())
            }
            None => self.fail(
                codes::SYNTAX,
                "unterminated comment",
                start,
                self.bytes.len(),
            ),
        }
    }

    fn processing_instruction(&mut self) -> PResult<()> {
        let start = self.pos;
        self.pos += 2;
        match self.text[self.pos..].find("?>") {
            Some(end) => {
                self.pos += end + 2;
                Ok(())
            }
            None => self.fail(
                codes::SYNTAX,
                "unterminated processing instruction",
                start,
                self.bytes.len(),
            ),
        }
    }

    fn name(&mut self) -> PResult<(String, Span)> {
        let start = self.pos;
        let mut chars = self.text[self.pos..].char_indices();
        match chars.next() {
            Some((_, c)) if is_name_start(c) => {}
            _ => return self.unexpected("a name"),
        }
        let mut end = self.text.len();
        for (i, c) in chars {
            if !is_name_char(c) {
                end = self.pos + i;
                break;
            }
        }
        self.pos = end;
        Ok((self.text[start..end].to_owned(), self.span(start, end)))
    }

    /// Reads one element and everything inside it.
    fn element(&mut self) -> PResult<Element> {
        let mut stack: Vec<Open> = Vec::new();
        let mut open = self.start_tag(&[])?;
        loop {
            let (start, depth) = match &open {
                StartTag::Empty(element) => (element.span.start() as usize, stack.len() + 1),
                StartTag::Open(element) => (element.start, stack.len() + 1),
            };
            if depth > self.max_depth as usize {
                let end = self.pos;
                return self.fail(
                    codes::DEPTH_LIMIT,
                    format!("elements nest more than {} deep, the limit", self.max_depth),
                    start,
                    end,
                );
            }
            match open {
                StartTag::Empty(element) => match stack.last_mut() {
                    Some(parent) => parent.children.push(Content::Element(element)),
                    None => return Ok(element),
                },
                StartTag::Open(element) => stack.push(*element),
            }
            // Content of the innermost open element, up to its next child
            // or its end tag.
            loop {
                let parent = stack.last_mut().expect("content is read inside an element");
                let start = self.pos;
                if self.at_end() {
                    let raw = parent.raw_name.clone();
                    let begin = parent.start;
                    return self.fail(
                        codes::SYNTAX,
                        format!("element <{raw}> is not closed"),
                        begin,
                        start,
                    );
                }
                if self.starts_with("</") {
                    let element = stack.pop().expect("an element is open");
                    let closed = self.end_tag(element)?;
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(Content::Element(closed)),
                        None => return Ok(closed),
                    }
                } else if self.starts_with("<!--") {
                    self.comment()?;
                } else if self.starts_with("<![CDATA[") {
                    let text = self.cdata()?;
                    let span = self.span(start, self.pos);
                    push_text(&mut stack, text, span);
                } else if self.starts_with("<?") {
                    self.processing_instruction()?;
                } else if self.starts_with("<!") {
                    return self.unexpected("an element, comment or CDATA section");
                } else if self.peek() == Some(b'<') {
                    let namespaces = stack
                        .last()
                        .map(|p| p.namespaces.clone())
                        .unwrap_or_default();
                    open = self.start_tag(&namespaces)?;
                    break;
                } else {
                    let text = self.character_data()?;
                    let span = self.span(start, self.pos);
                    push_text(&mut stack, text, span);
                }
            }
        }
    }

    fn start_tag(&mut self, inherited: &[(String, String)]) -> PResult<StartTag> {
        let start = self.pos;
        self.count_node(start)?;
        self.expect("<")?;
        let (raw_name, name_span) = self.name()?;
        let mut raw_attributes: Vec<RawAttribute> = Vec::new();
        loop {
            let spaced = self.skip_whitespace();
            match self.peek() {
                Some(b'>') => {
                    self.pos += 1;
                    break;
                }
                Some(b'/') => break,
                Some(_) if spaced => {
                    let attribute = self.attribute()?;
                    if raw_attributes
                        .iter()
                        .any(|a| a.raw_name == attribute.raw_name)
                    {
                        let (s, e) = (
                            attribute.span.start() as usize,
                            attribute.span.end() as usize,
                        );
                        return self.fail(
                            codes::DUPLICATE_ATTRIBUTE,
                            format!("attribute {} appears twice", attribute.raw_name),
                            s,
                            e,
                        );
                    }
                    raw_attributes.push(attribute);
                }
                _ => return self.unexpected("an attribute, `>` or `/>`"),
            }
        }
        let empty = self.starts_with("/>");
        if empty {
            self.pos += 2;
        }
        // Namespace declarations on this element come first, so the
        // innermost binding of a prefix is found first.
        let mut namespaces: Vec<(String, String)> = raw_attributes
            .iter()
            .filter_map(|a| {
                if a.raw_name == "xmlns" {
                    Some((String::new(), a.value.clone()))
                } else {
                    a.raw_name
                        .strip_prefix("xmlns:")
                        .map(|p| (p.to_owned(), a.value.clone()))
                }
            })
            .collect();
        let inherited: Vec<(String, String)> = inherited
            .iter()
            .filter(|(p, _)| !namespaces.iter().any(|(q, _)| q == p))
            .cloned()
            .collect();
        namespaces.extend(inherited);
        let (prefix, local) = split_name(&raw_name);
        let (prefix, local) = (prefix.map(str::to_owned), local.to_owned());
        let open = Open {
            raw_name,
            prefix,
            local,
            raw_attributes,
            children: Vec::new(),
            namespaces,
            start,
            name_span,
        };
        if empty {
            let end = self.pos;
            Ok(StartTag::Empty(self.finish(open, end)?))
        } else {
            Ok(StartTag::Open(Box::new(open)))
        }
    }

    fn attribute(&mut self) -> PResult<RawAttribute> {
        let start = self.pos;
        let (raw_name, _) = self.name()?;
        self.skip_whitespace();
        self.expect("=")?;
        self.skip_whitespace();
        let Some(quote @ (b'"' | b'\'')) = self.peek() else {
            return self.unexpected("a quoted attribute value");
        };
        self.pos += 1;
        let value_start = self.pos;
        let mut value = String::new();
        loop {
            match self.peek() {
                None => return self.unexpected("the end of the attribute value"),
                Some(q) if q == quote => break,
                Some(b'<') => return self.unexpected("an attribute value without `<`"),
                Some(b'&') => value.push_str(&self.reference()?),
                Some(b'\t' | b'\n' | b'\r') => {
                    // Attribute-value normalization.
                    if self.starts_with("\r\n") {
                        self.pos += 1;
                    }
                    value.push(' ');
                    self.pos += 1;
                }
                Some(_) => {
                    let c = self.text[self.pos..].chars().next().unwrap_or('\0');
                    value.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
        let value_end = self.pos;
        self.pos += 1;
        Ok(RawAttribute {
            raw_name,
            value,
            span: self.span(start, self.pos),
            value_span: self.span(value_start, value_end),
        })
    }

    /// `&name;`, `&#N;` or `&#xH;` at the cursor, expanded.
    fn reference(&mut self) -> PResult<String> {
        let start = self.pos;
        let Some(end) = self.text[self.pos..].find(';').filter(|&i| i <= 12) else {
            return self.fail(
                codes::ENTITY,
                "`&` must start a reference such as `&amp;`",
                start,
                start + 1,
            );
        };
        let body = &self.text[self.pos + 1..self.pos + end];
        let expanded = match body {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => {
                let number = if let Some(hex) = body.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(decimal) = body.strip_prefix('#') {
                    decimal.parse::<u32>().ok()
                } else {
                    let message = format!(
                        "entity &{body}; is not defined: only the predefined entities and character references are, as DTDs are not processed"
                    );
                    return self.fail(codes::ENTITY, message, start, start + end + 1);
                };
                number.and_then(char::from_u32).filter(|&c| is_xml_char(c))
            }
        };
        let Some(c) = expanded else {
            return self.fail(
                codes::ENTITY,
                format!("&{body}; is not a valid character reference"),
                start,
                start + end + 1,
            );
        };
        self.pos += end + 1;
        Ok(c.to_string())
    }

    fn cdata(&mut self) -> PResult<String> {
        let start = self.pos;
        self.pos += "<![CDATA[".len();
        match self.text[self.pos..].find("]]>") {
            Some(end) => {
                let text = normalize_newlines(&self.text[self.pos..self.pos + end]);
                self.pos += end + 3;
                Ok(text)
            }
            None => self.fail(
                codes::SYNTAX,
                "unterminated CDATA section",
                start,
                self.bytes.len(),
            ),
        }
    }

    fn character_data(&mut self) -> PResult<String> {
        let mut out = String::new();
        while let Some(b) = self.peek() {
            match b {
                b'<' => break,
                b'&' => out.push_str(&self.reference()?),
                _ => {
                    let rest = &self.text[self.pos..];
                    let run = rest.find(['<', '&']).unwrap_or(rest.len());
                    out.push_str(&normalize_newlines(&rest[..run]));
                    self.pos += run;
                }
            }
        }
        Ok(out)
    }

    fn end_tag(&mut self, open: Open) -> PResult<Element> {
        let start = self.pos;
        self.pos += 2;
        let (name, _) = self.name()?;
        if name != open.raw_name {
            let end = self.pos;
            return self.fail(
                codes::MISMATCHED_TAG,
                format!("</{name}> closes <{}>", open.raw_name),
                start,
                end,
            );
        }
        self.skip_whitespace();
        self.expect(">")?;
        let end = self.pos;
        self.finish(open, end)
    }

    /// Resolves an element's names now that its scope is known.
    fn finish(&mut self, open: Open, end: usize) -> PResult<Element> {
        let lookup = |prefix: &str| -> Option<Option<String>> {
            if prefix == "xml" {
                return Some(Some(XML_NAMESPACE.to_owned()));
            }
            open.namespaces
                .iter()
                .find(|(p, _)| p == prefix)
                .map(|(_, uri)| Some(uri.clone()).filter(|u| !u.is_empty()))
        };
        let namespace = match &open.prefix {
            Some(prefix) => {
                let Some(Some(uri)) = lookup(prefix) else {
                    let span = open.name_span;
                    return self.fail(
                        codes::UNBOUND_PREFIX,
                        format!("namespace prefix `{prefix}` is not declared"),
                        span.start() as usize,
                        span.end() as usize,
                    );
                };
                Some(uri)
            }
            None => lookup("").flatten(),
        };
        let mut attributes: Vec<Attribute> = Vec::new();
        for raw in open.raw_attributes {
            if raw.raw_name == "xmlns" || raw.raw_name.starts_with("xmlns:") {
                continue;
            }
            let (prefix, local) = split_name(&raw.raw_name);
            // Unprefixed attributes are in no namespace.
            let attribute_namespace = match prefix {
                Some(prefix) => match lookup(prefix) {
                    Some(Some(uri)) => Some(uri),
                    _ => {
                        return self.fail(
                            codes::UNBOUND_PREFIX,
                            format!("namespace prefix `{prefix}` is not declared"),
                            raw.span.start() as usize,
                            raw.span.end() as usize,
                        );
                    }
                },
                None => None,
            };
            let name = QName {
                namespace: attribute_namespace,
                local: local.to_owned(),
            };
            if attributes.iter().any(|a| a.name == name) {
                return self.fail(
                    codes::DUPLICATE_ATTRIBUTE,
                    format!("attribute {name} appears twice"),
                    raw.span.start() as usize,
                    raw.span.end() as usize,
                );
            }
            attributes.push(Attribute {
                name,
                value: raw.value,
                span: raw.span,
                value_span: raw.value_span,
            });
        }
        Ok(Element {
            name: QName {
                namespace,
                local: open.local,
            },
            raw_name: open.raw_name,
            attributes,
            children: open.children,
            namespaces: open.namespaces,
            span: self.span(open.start, end),
            name_span: open.name_span,
        })
    }
}

enum StartTag {
    Empty(Element),
    Open(Box<Open>),
}

/// Adds text to the innermost open element, merging with text before it
/// (character data interrupted by a comment is one run) and dropping runs
/// that are only whitespace between elements.
fn push_text(stack: &mut [Open], text: String, span: Span) {
    let parent = stack.last_mut().expect("text is read inside an element");
    if let Some(Content::Text(previous, previous_span)) = parent.children.last_mut() {
        previous.push_str(&text);
        *previous_span = previous_span.join(span).unwrap_or(*previous_span);
        return;
    }
    if !text.trim().is_empty() {
        parent.children.push(Content::Text(text, span));
    }
}

fn split_name(name: &str) -> (Option<&str>, &str) {
    match name.split_once(':') {
        Some((prefix, local)) if !prefix.is_empty() && !local.is_empty() => (Some(prefix), local),
        _ => (None, name),
    }
}

fn normalize_newlines(text: &str) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_owned()
    }
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == ':' || (!c.is_ascii() && c.is_alphabetic())
}

fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || c.is_ascii_digit()
        || matches!(c, '-' | '.')
        || (!c.is_ascii() && c.is_alphanumeric())
}

/// Characters XML 1.0 allows in documents.
fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || (c >= ' ' && c != '\u{fffe}' && c != '\u{ffff}')
}

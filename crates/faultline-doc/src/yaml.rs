//! A YAML 1.2 loader that lowers the event stream of `saphyr-parser` into the
//! spanned [`Node`] tree.
//!
//! Only the JSON-compatible subset of YAML reaches consumers. Everything else
//! is resolved here:
//!
//! * **Scalars** are typed with the YAML 1.2 core schema (so `yes` is a string
//!   and `0o17` an integer) or an explicit core tag such as `!!str`.
//! * **Mapping keys** are always strings. A plain `200:` key becomes `"200"`,
//!   which is what OpenAPI response maps rely on. Collections as keys are
//!   rejected because they have no JSON equivalent.
//! * **Anchors and aliases** are expanded. Every copy counts towards the node
//!   limit and the nesting limit, which is what defeats "billion laughs"
//!   documents: the expansion is bounded before it is materialized.
//! * **Merge keys** (`<<`) from YAML 1.1 are supported because they are common
//!   in hand-written API descriptions; explicit keys win over merged ones.
//! * **Multiple documents** in one stream are an error: a schema file is one
//!   document, and silently ignoring the rest could hide content.

use std::collections::HashMap;

use faultline_source::{Diagnostic, Diagnostics, Limits, SourceId, Span};
use saphyr_parser::{Event, Parser, ScalarStyle, ScanError, Tag};

use crate::codes;
use crate::node::{Node, Object, Value};
use crate::number::{Decimal, DecimalError, Number};

const CORE_TAG_HANDLE: &str = "tag:yaml.org,2002:";

/// Marker for a fatal error whose diagnostic has already been recorded.
struct Fatal;

type LResult<T> = Result<T, Fatal>;

pub(crate) fn load(
    text: &str,
    source: SourceId,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Option<Node> {
    load_keeping(text, source, limits, diagnostics, &[]).0
}

/// Loads `text`, keeping scalars tagged with one of the local tags in `keep`
/// (such as RAML's `!include`) as strings and reporting where they are,
/// instead of warning that the tags are unsupported.
pub(crate) fn load_keeping(
    text: &str,
    source: SourceId,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
    keep: &[&str],
) -> (Option<Node>, Vec<crate::LocalTag>) {
    let mut loader = Loader {
        keep,
        kept: Vec::new(),
        offsets: CharOffsets::new(text),
        source,
        stack: Vec::new(),
        anchors: HashMap::new(),
        root: None,
        documents: 0,
        nodes: 0,
        max_nodes: limits.max_document_nodes,
        max_depth: limits.max_depth.min(crate::HARD_MAX_DEPTH),
        diagnostics,
    };
    let mut parser = Parser::new_from_str(text);
    while let Some(event) = parser.next_event() {
        let result = match event {
            Ok((event, span)) => loader.event(event, span),
            Err(error) => loader.scan_error(&error),
        };
        if result.is_err() {
            return (None, loader.kept);
        }
    }
    let root = loader
        .root
        .unwrap_or_else(|| Node::new(Value::Null, Span::empty(source, 0)));
    (Some(root), loader.kept)
}

/// A fully built value on its way to its parent.
struct Built {
    node: Node,
    /// Where this value occurs in the text. Differs from `node.span` for
    /// aliases, whose node keeps the span of the anchored original.
    site: Span,
    /// The key this value produces when used as a mapping key; `None` for
    /// collections.
    key: Option<Key>,
    /// Number of nodes in the subtree, including the root.
    size: u64,
    /// Collection nesting depth of the subtree (0 for scalars).
    height: u32,
}

#[derive(Clone)]
struct Key {
    text: String,
    /// A plain, untagged `<<` — the merge key.
    is_merge: bool,
}

/// An anchored value available for aliasing.
struct Anchored {
    node: Node,
    key: Option<Key>,
    size: u64,
    height: u32,
}

struct Frame {
    kind: FrameKind,
    anchor: usize,
    start: u32,
    flow: bool,
    /// End of the last child occurrence, used as the end of block collections,
    /// whose end events carry no useful position.
    last_end: u32,
    nodes_at_start: u64,
    child_height: u32,
}

enum FrameKind {
    Sequence(Vec<Node>),
    Mapping {
        object: Object,
        pending_key: Option<(Key, Span)>,
        merges: Vec<Object>,
    },
}

struct Loader<'a> {
    /// Local tags the caller keeps.
    keep: &'a [&'a str],
    kept: Vec<crate::LocalTag>,
    offsets: CharOffsets<'a>,
    source: SourceId,
    stack: Vec<Frame>,
    anchors: HashMap<usize, Anchored>,
    root: Option<Node>,
    documents: u32,
    nodes: u64,
    max_nodes: u64,
    max_depth: u32,
    diagnostics: &'a mut Diagnostics,
}

impl Loader<'_> {
    fn event(&mut self, event: Event<'_>, span: saphyr_parser::Span) -> LResult<()> {
        let span = self.span(span);
        match event {
            Event::Nothing | Event::StreamStart | Event::StreamEnd | Event::DocumentEnd => Ok(()),
            Event::DocumentStart(_) => {
                self.documents += 1;
                if self.documents > 1 {
                    return self.fail(
                        Diagnostic::error("file contains more than one YAML document")
                            .with_code(codes::YAML_MULTIPLE_DOCUMENTS)
                            .with_primary(span, "second document starts here")
                            .with_help("split the documents into separate files"),
                    );
                }
                Ok(())
            }
            Event::Scalar(value, style, anchor, tag) => {
                let built = self.scalar(&value, style, tag.as_deref(), span)?;
                self.remember(anchor, &built);
                self.push(built)
            }
            Event::Alias(id) => self.alias(id, span),
            Event::SequenceStart(anchor, tag) => {
                let tag = tag.as_deref().map(normalize_verbatim);
                self.check_collection_tag(tag.as_ref(), "seq", span);
                self.open(FrameKind::Sequence(Vec::new()), anchor, span)
            }
            Event::MappingStart(anchor, tag) => {
                let tag = tag.as_deref().map(normalize_verbatim);
                self.check_collection_tag(tag.as_ref(), "map", span);
                let kind = FrameKind::Mapping {
                    object: Object::new(),
                    pending_key: None,
                    merges: Vec::new(),
                };
                self.open(kind, anchor, span)
            }
            Event::SequenceEnd | Event::MappingEnd => self.close(span),
        }
    }

    fn open(&mut self, kind: FrameKind, anchor: usize, span: Span) -> LResult<()> {
        if self.stack.len() as u32 >= self.max_depth {
            return self.fail(
                Diagnostic::error("document is nested too deeply")
                    .with_code(codes::DEPTH_LIMIT)
                    .with_primary(
                        span,
                        format!("exceeds the nesting limit of {}", self.max_depth),
                    ),
            );
        }
        let nodes_at_start = self.nodes;
        self.count_nodes(1, span)?;
        let flow = matches!(
            self.offsets.text.as_bytes().get(span.start() as usize),
            Some(b'[' | b'{')
        );
        self.stack.push(Frame {
            kind,
            anchor,
            start: span.start(),
            flow,
            last_end: span.start(),
            nodes_at_start,
            child_height: 0,
        });
        Ok(())
    }

    fn close(&mut self, end_span: Span) -> LResult<()> {
        let frame = self
            .stack
            .pop()
            .expect("parser emits balanced collection events");
        let end = if frame.flow {
            end_span.end()
        } else {
            frame.last_end
        };
        let span = Span::new(self.source, frame.start, end.max(frame.start));
        let value = match frame.kind {
            FrameKind::Sequence(items) => Value::Array(items),
            FrameKind::Mapping {
                mut object, merges, ..
            } => {
                for merged in merges {
                    for (key, member) in merged.members() {
                        if !object.contains_key(key) {
                            object.insert(key.to_owned(), member.key_span, member.value.clone());
                        }
                    }
                }
                Value::Object(object)
            }
        };
        let built = Built {
            node: Node::new(value, span),
            site: span,
            key: None,
            size: self.nodes - frame.nodes_at_start,
            height: frame.child_height + 1,
        };
        self.remember(frame.anchor, &built);
        self.push(built)
    }

    fn alias(&mut self, id: usize, span: Span) -> LResult<()> {
        let Some(anchored) = self.anchors.get(&id) else {
            return self.fail(
                Diagnostic::error("alias refers to an undefined anchor")
                    .with_code(codes::YAML_UNDEFINED_ALIAS)
                    .with_primary(span, "no anchor with this name precedes the alias"),
            );
        };
        let (size, height) = (anchored.size, anchored.height);
        if self.stack.len() as u32 + height > self.max_depth {
            return self.fail(
                Diagnostic::error("alias expansion is nested too deeply")
                    .with_code(codes::DEPTH_LIMIT)
                    .with_primary(
                        span,
                        format!(
                            "expanding this alias exceeds the nesting limit of {}",
                            self.max_depth
                        ),
                    ),
            );
        }
        self.count_nodes(size, span)?;
        let anchored = &self.anchors[&id];
        let built = Built {
            node: anchored.node.clone(),
            site: span,
            key: anchored.key.clone(),
            size,
            height,
        };
        self.push(built)
    }

    fn remember(&mut self, anchor: usize, built: &Built) {
        if anchor != 0 {
            // Re-defining an anchor name is legal YAML; later aliases refer to
            // the most recent definition, and the parser assigns a new id.
            self.anchors.insert(
                anchor,
                Anchored {
                    node: built.node.clone(),
                    key: built.key.clone(),
                    size: built.size,
                    height: built.height,
                },
            );
        }
    }

    /// Hands a finished value to the enclosing collection, or makes it the
    /// document root.
    fn push(&mut self, built: Built) -> LResult<()> {
        let Some(frame) = self.stack.last_mut() else {
            self.root = Some(built.node);
            return Ok(());
        };
        frame.last_end = frame.last_end.max(built.site.end());
        frame.child_height = frame.child_height.max(built.height);
        match &mut frame.kind {
            FrameKind::Sequence(items) => {
                items.push(built.node);
                Ok(())
            }
            FrameKind::Mapping {
                object,
                pending_key,
                merges,
            } => match pending_key.take() {
                None => {
                    let Some(key) = built.key else {
                        return self.fail(
                            Diagnostic::error("mapping keys must be scalars")
                                .with_code(codes::YAML_COMPLEX_KEY)
                                .with_primary(built.site, "collection used as a key")
                                .with_note("keys must be strings for the document to have a JSON equivalent"),
                        );
                    };
                    *pending_key = Some((key, built.site));
                    Ok(())
                }
                Some((key, key_span)) if key.is_merge => {
                    match built.node.value {
                        Value::Object(obj) => merges.push(obj),
                        Value::Array(items)
                            if items.iter().all(|n| matches!(n.value, Value::Object(_))) =>
                        {
                            merges.extend(items.into_iter().filter_map(|n| match n.value {
                                Value::Object(obj) => Some(obj),
                                _ => None,
                            }));
                        }
                        _ => {
                            self.diagnostics.push(
                                Diagnostic::error(
                                    "merge key value must be a mapping or a list of mappings",
                                )
                                .with_code(codes::YAML_INVALID_MERGE)
                                .with_primary(built.site, "not a mapping")
                                .with_secondary(key_span, "merge key"),
                            );
                        }
                    }
                    Ok(())
                }
                Some((key, key_span)) => {
                    let name = key.text;
                    if let Some(previous) = object.insert(name.clone(), key_span, built.node) {
                        self.diagnostics.push(
                            Diagnostic::error(format!("duplicate key \"{name}\""))
                                .with_code(codes::DUPLICATE_KEY)
                                .with_primary(key_span, "duplicate key")
                                .with_secondary(previous.key_span, "first defined here")
                                .with_note("duplicate keys make the document ambiguous; parsers disagree on which value wins"),
                        );
                    }
                    Ok(())
                }
            },
        }
    }

    fn scalar(
        &mut self,
        text: &str,
        style: ScalarStyle,
        tag: Option<&Tag>,
        span: Span,
    ) -> LResult<Built> {
        self.count_nodes(1, span)?;
        let tag = tag.map(normalize_verbatim);
        let tag = tag.as_ref();
        let plain = style == ScalarStyle::Plain;
        let value = match tag {
            None if plain => self.resolve_plain(text, span),
            None => Value::String(text.to_owned()),
            Some(tag) if is_non_specific(tag) => Value::String(text.to_owned()),
            Some(tag) if tag.is_yaml_core_schema() => self.resolve_tagged(text, &tag.suffix, span),
            Some(tag)
                if self
                    .keep
                    .contains(&format!("{}{}", tag.handle, tag.suffix).as_str()) =>
            {
                self.kept.push(crate::LocalTag {
                    tag: format!("{}{}", tag.handle, tag.suffix),
                    span,
                });
                Value::String(text.to_owned())
            }
            Some(tag) => {
                self.unsupported_tag(tag, span);
                if plain {
                    self.resolve_plain(text, span)
                } else {
                    Value::String(text.to_owned())
                }
            }
        };
        Ok(Built {
            node: Node::new(value, span),
            site: span,
            key: Some(Key {
                text: text.to_owned(),
                is_merge: plain && tag.is_none() && text == "<<",
            }),
            size: 1,
            height: 0,
        })
    }

    /// Resolves an untagged plain scalar with the YAML 1.2 core schema.
    fn resolve_plain(&mut self, text: &str, span: Span) -> Value {
        match text {
            "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
            "true" | "True" | "TRUE" => return Value::Bool(true),
            "false" | "False" | "FALSE" => return Value::Bool(false),
            _ => {}
        }
        match core_number(text) {
            Some(Ok(number)) => Value::Number(number),
            Some(Err(DecimalError::ExponentOutOfRange)) => {
                self.number_out_of_range(span);
                Value::String(text.to_owned())
            }
            Some(Err(DecimalError::Syntax)) | None => Value::String(text.to_owned()),
        }
    }

    /// Resolves a scalar with an explicit `!!type` tag from the core schema.
    fn resolve_tagged(&mut self, text: &str, suffix: &str, span: Span) -> Value {
        let resolved = match suffix {
            "str" => Some(Value::String(text.to_owned())),
            "null" => matches!(text, "" | "~" | "null" | "Null" | "NULL").then_some(Value::Null),
            "bool" => match text {
                "true" | "True" | "TRUE" => Some(Value::Bool(true)),
                "false" | "False" | "FALSE" => Some(Value::Bool(false)),
                _ => None,
            },
            "int" => match core_number(text) {
                Some(Ok(Number::Finite(d)))
                    if d.is_integer() && !text.contains(['.', 'e', 'E']) =>
                {
                    Some(Value::Number(Number::Finite(d)))
                }
                _ => None,
            },
            "float" => match core_number(text) {
                Some(Ok(n)) => Some(Value::Number(n)),
                Some(Err(DecimalError::ExponentOutOfRange)) => {
                    self.number_out_of_range(span);
                    return Value::String(text.to_owned());
                }
                _ => None,
            },
            _ => {
                self.unsupported_tag(
                    &Tag {
                        handle: CORE_TAG_HANDLE.to_owned(),
                        suffix: suffix.to_owned(),
                    },
                    span,
                );
                return Value::String(text.to_owned());
            }
        };
        resolved.unwrap_or_else(|| {
            self.diagnostics.push(
                Diagnostic::error(format!("value does not match its !!{suffix} tag"))
                    .with_code(codes::YAML_TAG_MISMATCH)
                    .with_primary(span, format!("not a valid {suffix}")),
            );
            Value::String(text.to_owned())
        })
    }

    fn check_collection_tag(&mut self, tag: Option<&Tag>, expected: &str, span: Span) {
        if let Some(tag) = tag
            && !is_non_specific(tag)
            && !(tag.is_yaml_core_schema() && tag.suffix == expected)
        {
            self.unsupported_tag(tag, span);
        }
    }

    fn unsupported_tag(&mut self, tag: &Tag, span: Span) {
        let shown = if tag.is_yaml_core_schema() {
            format!("!!{}", tag.suffix)
        } else if tag.handle.is_empty() {
            format!("!<{}>", tag.suffix)
        } else {
            tag.to_string()
        };
        self.diagnostics.push(
            Diagnostic::warning(format!("unsupported YAML tag {shown}"))
                .with_code(codes::YAML_UNSUPPORTED_TAG)
                .with_primary(span, "tag ignored")
                .with_note("only the core schema tags !!str, !!int, !!float, !!bool, !!null, !!seq and !!map are understood"),
        );
    }

    fn number_out_of_range(&mut self, span: Span) {
        self.diagnostics.push(
            Diagnostic::error("number is out of range")
                .with_code(codes::NUMBER_OUT_OF_RANGE)
                .with_primary(span, "exponent is too large"),
        );
    }

    fn count_nodes(&mut self, n: u64, span: Span) -> LResult<()> {
        self.nodes = self.nodes.saturating_add(n);
        if self.nodes > self.max_nodes {
            return self.fail(
                Diagnostic::error("document has too many values")
                    .with_code(codes::NODE_LIMIT)
                    .with_primary(
                        span,
                        format!("exceeds the limit of {} values", self.max_nodes),
                    )
                    .with_note("aliases count once for every node they copy"),
            );
        }
        Ok(())
    }

    fn scan_error(&mut self, error: &ScanError) -> LResult<()> {
        let offset = self.offsets.byte_offset(error.marker().index());
        let end = self.offsets.next_char_end(offset);
        let span = Span::new(self.source, offset, end);
        let info = error.info();
        let diagnostic = if info.contains("recursion limit exceeded") {
            // The scanner tracks flow nesting in a u8 and gives up at 255,
            // which can be below the configured limit.
            Diagnostic::error("document is nested too deeply")
                .with_code(codes::DEPTH_LIMIT)
                .with_primary(span, "exceeds the YAML parser's flow nesting limit of 255")
        } else if info.contains("unknown anchor") {
            Diagnostic::error("alias refers to an undefined anchor")
                .with_code(codes::YAML_UNDEFINED_ALIAS)
                .with_primary(span, "no anchor with this name precedes the alias")
        } else {
            Diagnostic::error(format!("invalid YAML: {info}"))
                .with_code(codes::YAML_SYNTAX)
                .with_primary(span, "")
        };
        self.fail(diagnostic)
    }

    fn fail<T>(&mut self, diagnostic: Diagnostic) -> LResult<T> {
        self.diagnostics.push(diagnostic);
        Err(Fatal)
    }

    fn span(&self, span: saphyr_parser::Span) -> Span {
        let start = self.offsets.byte_offset(span.start.index());
        let end = self.offsets.byte_offset(span.end.index());
        Span::new(self.source, start, end.max(start))
    }
}

/// Rewrites a verbatim core tag (`!<tag:yaml.org,2002:str>`) into the
/// shorthand form (`!!str`) so both spellings are handled identically.
fn normalize_verbatim(tag: &Tag) -> Tag {
    match tag.suffix.strip_prefix(CORE_TAG_HANDLE) {
        Some(suffix) if tag.handle.is_empty() => Tag {
            handle: CORE_TAG_HANDLE.to_owned(),
            suffix: suffix.to_owned(),
        },
        _ => tag.clone(),
    }
}

/// Whether `tag` is the non-specific `!` tag, which forces a scalar to be a
/// string. The parser reports it (like the equivalent verbatim `!<!>`) as an
/// empty handle with suffix `!`.
fn is_non_specific(tag: &Tag) -> bool {
    tag.handle.is_empty() && tag.suffix == "!"
}

/// Parses the numeric forms of the YAML 1.2 core schema: decimal, octal
/// (`0o`) and hexadecimal (`0x`) integers, decimal floats, and the special
/// float values. Returns `None` for text that is not numeric at all.
fn core_number(text: &str) -> Option<Result<Number, DecimalError>> {
    if let Some(octal) = text.strip_prefix("0o") {
        return radix_integer(octal, 8);
    }
    if let Some(hex) = text.strip_prefix("0x") {
        return radix_integer(hex, 16);
    }
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    match unsigned {
        ".inf" | ".Inf" | ".INF" => {
            return Some(Ok(if text.starts_with('-') {
                Number::NegInfinity
            } else {
                Number::PosInfinity
            }));
        }
        ".nan" | ".NaN" | ".NAN" if unsigned.len() == text.len() => return Some(Ok(Number::NaN)),
        _ => {}
    }
    match Decimal::parse(text) {
        Ok(d) => Some(Ok(Number::Finite(d))),
        Err(DecimalError::Syntax) => None,
        Err(e @ DecimalError::ExponentOutOfRange) => Some(Err(e)),
    }
}

fn radix_integer(digits: &str, radix: u32) -> Option<Result<Number, DecimalError>> {
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    // Values beyond u128 do not fit the representation; treat them as the
    // strings they would be in a schema without integer resolution.
    u128::from_str_radix(digits, radix)
        .ok()
        .map(|v| Ok(Number::Finite(Decimal::from_u128(v))))
}

/// Converts the parser's character indices into byte offsets.
///
/// `saphyr-parser` reports positions in Unicode scalar values. For ASCII text
/// the two coincide; otherwise a checkpoint every [`STRIDE`](Self::STRIDE)
/// characters bounds each conversion to a short forward scan.
struct CharOffsets<'a> {
    text: &'a str,
    checkpoints: Option<Vec<u32>>,
}

impl<'a> CharOffsets<'a> {
    const STRIDE: usize = 64;

    fn new(text: &'a str) -> Self {
        let checkpoints = (!text.is_ascii()).then(|| {
            text.char_indices()
                .step_by(Self::STRIDE)
                .map(|(i, _)| i as u32)
                .collect()
        });
        Self { text, checkpoints }
    }

    fn byte_offset(&self, char_index: usize) -> u32 {
        let Some(checkpoints) = &self.checkpoints else {
            return char_index.min(self.text.len()) as u32;
        };
        let Some(&base) = checkpoints.get(char_index / Self::STRIDE) else {
            return self.text.len() as u32;
        };
        let rest = &self.text[base as usize..];
        let within = char_index % Self::STRIDE;
        rest.char_indices()
            .nth(within)
            .map_or(self.text.len() as u32, |(i, _)| base + i as u32)
    }

    /// End of the character starting at `offset`, for one-character error spans.
    fn next_char_end(&self, offset: u32) -> u32 {
        self.text[offset as usize..]
            .chars()
            .next()
            .map_or(offset, |c| offset + c.len_utf8() as u32)
    }
}

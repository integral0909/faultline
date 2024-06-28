//! Importing GraphQL client documents.
//!
//! The queries, mutations and subscriptions clients send say exactly which
//! fields and arguments they depend on. Each root field an operation
//! selects is a call of the operation `query/pets`; every field below it,
//! and every argument passed, is recorded by its dotted path
//! (`owner.name`, `nicknames().limit`, `()first` for root arguments).
//! Fragments are expanded where they are spread; type conditions are
//! ignored, so a field selected on any member counts. Aliases do not
//! matter, and argument values are never read.

use std::collections::HashMap;

use faultline_graphql::lexer::{Token, TokenKind, tokenize};
use faultline_ir::{Api, OperationKey};
use faultline_source::SourceId;

use crate::index::{OperationUsage, Source, UsageIndex};

/// Selections nest at most this deep; deeper documents are cut off there.
const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, Default)]
struct Selection {
    name: String,
    arguments: Vec<String>,
    children: Vec<Item>,
}

#[derive(Clone, Debug)]
enum Item {
    Field(Selection),
    Spread(String),
    Inline(Vec<Item>),
}

struct Operation {
    kind: String,
    items: Vec<Item>,
}

/// Imports client `documents` against the operations of `api`.
pub fn import<S: AsRef<str>>(api: &Api, documents: impl IntoIterator<Item = S>) -> UsageIndex {
    let mut index = UsageIndex::new(Source::GraphqlDocuments);
    for op in &api.operations {
        if let OperationKey::Rpc { .. } = op.key {
            index
                .operations
                .insert(op.key.identity(), OperationUsage::default());
        }
    }
    let mut fragments: HashMap<String, Vec<Item>> = HashMap::new();
    let mut operations = Vec::new();
    for text in documents {
        let (ops, frags) = parse(text.as_ref());
        operations.extend(ops);
        fragments.extend(frags);
    }
    for operation in operations {
        for item in flatten(&operation.items, &fragments, 0) {
            let identity = format!("{}/{}", operation.kind, item.name);
            let Some(usage) = index.operations.get_mut(&identity) else {
                index.unmatched += 1;
                continue;
            };
            usage.calls += 1;
            for argument in &item.arguments {
                *usage.fields.entry(format!("(){argument}")).or_default() += 1;
            }
            record(&item.children, "", &fragments, usage, 1);
        }
    }
    index
}

/// Records the fields and arguments under `prefix`.
fn record(
    items: &[Item],
    prefix: &str,
    fragments: &HashMap<String, Vec<Item>>,
    usage: &mut OperationUsage,
    depth: usize,
) {
    if depth > MAX_DEPTH {
        return;
    }
    for field in flatten(items, fragments, depth) {
        if field.name == "__typename" {
            continue;
        }
        let path = if prefix.is_empty() {
            field.name.clone()
        } else {
            format!("{prefix}.{}", field.name)
        };
        *usage.fields.entry(path.clone()).or_default() += 1;
        for argument in &field.arguments {
            *usage
                .fields
                .entry(format!("{path}().{argument}"))
                .or_default() += 1;
        }
        record(&field.children, &path, fragments, usage, depth + 1);
    }
}

/// The fields of `items`, with fragments expanded.
fn flatten(items: &[Item], fragments: &HashMap<String, Vec<Item>>, depth: usize) -> Vec<Selection> {
    let mut out = Vec::new();
    if depth > MAX_DEPTH {
        return out;
    }
    for item in items {
        match item {
            Item::Field(field) => out.push(field.clone()),
            Item::Inline(inner) => out.extend(flatten(inner, fragments, depth + 1)),
            Item::Spread(name) => {
                if let Some(inner) = fragments.get(name) {
                    out.extend(flatten(inner, fragments, depth + 1));
                }
            }
        }
    }
    out
}

/// Operations and fragments of one document. Malformed parts are skipped:
/// usage import is best effort, and the documents belong to clients.
fn parse(text: &str) -> (Vec<Operation>, HashMap<String, Vec<Item>>) {
    let mut parser = Parser {
        tokens: tokenize(SourceId::from_raw(0), text),
        pos: 0,
    };
    let mut operations = Vec::new();
    let mut fragments = HashMap::new();
    while parser.peek() != &TokenKind::Eof {
        let start = parser.pos;
        match parser.peek().clone() {
            TokenKind::Punct('{') => {
                let items = parser.selection_set(0);
                operations.push(Operation {
                    kind: "query".into(),
                    items,
                });
            }
            TokenKind::Name(word)
                if matches!(word.as_str(), "query" | "mutation" | "subscription") =>
            {
                parser.bump();
                parser.skip_until_selection_set();
                let items = parser.selection_set(0);
                operations.push(Operation { kind: word, items });
            }
            TokenKind::Name(word) if word == "fragment" => {
                parser.bump();
                let name = parser.name();
                parser.skip_until_selection_set();
                let items = parser.selection_set(0);
                if let Some(name) = name {
                    fragments.insert(name, items);
                }
            }
            _ => {}
        }
        if parser.pos == start {
            parser.bump();
        }
    }
    (operations, fragments)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos.min(self.tokens.len() - 1)].kind
    }

    fn bump(&mut self) {
        if self.peek() != &TokenKind::Eof {
            self.pos += 1;
        }
    }

    fn name(&mut self) -> Option<String> {
        if let TokenKind::Name(name) = self.peek().clone() {
            self.bump();
            Some(name)
        } else {
            None
        }
    }

    /// Skips a name, variable definitions and directives up to `{`.
    fn skip_until_selection_set(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.peek() {
                TokenKind::Eof => return,
                TokenKind::Punct('{') if depth == 0 => return,
                TokenKind::Punct('(' | '[') => depth += 1,
                TokenKind::Punct(')' | ']') => depth = depth.saturating_sub(1),
                _ => {}
            }
            self.bump();
        }
    }

    /// Skips a balanced `( ... )`, returning the argument names at its top
    /// level: the names directly followed by `:`.
    fn arguments(&mut self) -> Vec<String> {
        let mut names = Vec::new();
        self.bump();
        let mut depth = 1usize;
        while depth > 0 {
            match self.peek().clone() {
                TokenKind::Eof => break,
                TokenKind::Punct('(' | '[' | '{') => depth += 1,
                TokenKind::Punct(')' | ']' | '}') => depth -= 1,
                TokenKind::Name(name)
                    if depth == 1
                        && self.tokens.get(self.pos + 1).map(|t| &t.kind)
                            == Some(&TokenKind::Punct(':')) =>
                {
                    names.push(name);
                }
                _ => {}
            }
            self.bump();
        }
        names
    }

    fn directives(&mut self) {
        while self.peek() == &TokenKind::Punct('@') {
            self.bump();
            self.bump();
            if self.peek() == &TokenKind::Punct('(') {
                self.arguments();
            }
        }
    }

    fn selection_set(&mut self, depth: usize) -> Vec<Item> {
        let mut items = Vec::new();
        if self.peek() != &TokenKind::Punct('{') {
            return items;
        }
        self.bump();
        loop {
            match self.peek().clone() {
                TokenKind::Eof => return items,
                TokenKind::Punct('}') => {
                    self.bump();
                    return items;
                }
                TokenKind::Spread => {
                    self.bump();
                    match self.peek().clone() {
                        TokenKind::Name(name) if name != "on" => {
                            self.bump();
                            self.directives();
                            items.push(Item::Spread(name));
                        }
                        _ => {
                            if self.peek() == &TokenKind::Name("on".into()) {
                                self.bump();
                                self.bump();
                            }
                            self.directives();
                            let inner = if depth < MAX_DEPTH {
                                self.selection_set(depth + 1)
                            } else {
                                self.skip_block();
                                Vec::new()
                            };
                            items.push(Item::Inline(inner));
                        }
                    }
                }
                TokenKind::Name(first) => {
                    self.bump();
                    // `alias: name`
                    let name = if self.peek() == &TokenKind::Punct(':') {
                        self.bump();
                        self.name().unwrap_or(first)
                    } else {
                        first
                    };
                    let arguments = if self.peek() == &TokenKind::Punct('(') {
                        self.arguments()
                    } else {
                        Vec::new()
                    };
                    self.directives();
                    let children = if self.peek() == &TokenKind::Punct('{') {
                        if depth < MAX_DEPTH {
                            self.selection_set(depth + 1)
                        } else {
                            self.skip_block();
                            Vec::new()
                        }
                    } else {
                        Vec::new()
                    };
                    items.push(Item::Field(Selection {
                        name,
                        arguments,
                        children,
                    }));
                }
                _ => self.bump(),
            }
        }
    }

    fn skip_block(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.peek() {
                TokenKind::Eof => return,
                TokenKind::Punct('{') => depth += 1,
                TokenKind::Punct('}') => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        self.bump();
                        return;
                    }
                }
                _ => {}
            }
            self.bump();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_named_without_values() {
        let (ops, _) = parse(
            r#"{ pets(first: 10, filter: {species: CAT, tags: ["a"]}, after: $cursor) { name } }"#,
        );
        let Item::Field(pets) = &ops[0].items[0] else {
            panic!()
        };
        assert_eq!(pets.arguments, ["first", "filter", "after"]);
    }

    #[test]
    fn operations_fragments_and_aliases() {
        let (ops, fragments) = parse(
            r"
            query Pets($n: Int = 5) @cached { mine: pets(first: $n) { ...PetFields } }
            mutation { addPet(input: {name: x}) { id } }
            fragment PetFields on Pet { name owner { ... on Person { name } } }
            ",
        );
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].kind, "query");
        assert_eq!(ops[1].kind, "mutation");
        let Item::Field(pets) = &ops[0].items[0] else {
            panic!()
        };
        assert_eq!(pets.name, "pets", "aliases are resolved to the field");
        assert!(fragments.contains_key("PetFields"));
    }
}

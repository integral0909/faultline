//! Reading schema documents into [`Components`].
//!
//! Documents are loaded through the workspace [`Loader`], so includes and
//! imports are confined to the workspace like every other reference. A
//! schema without a target namespace included into one with a namespace
//! takes on the includer's ("chameleon" include).

use std::collections::HashSet;

use faultline_doc::UriRef;
use faultline_doc::loader::Loader;
use faultline_source::{Diagnostic, Diagnostics, Limits, SourceMap, Span};
use faultline_xml::Element;

use crate::codes;
use crate::model::{
    AttributeUse, ComplexType, Components, ContentModel, Derivation, ElementDecl, Facets, Occurs,
    Particle, QName, SimpleType, SimpleVariety, Term, TypeDef, TypeRef,
};

/// The XML Schema namespace.
pub const XS: &str = "http://www.w3.org/2001/XMLSchema";

/// At most this many schema documents are read for one API.
const MAX_DOCUMENTS: usize = 1000;

/// Where a document without a target namespace takes its namespace from.
#[derive(Clone)]
enum Inherit {
    /// It has none (a document loaded on its own, or imported).
    Own,
    /// The includer's (a "chameleon" include).
    From(Option<String>),
}

/// Per-document settings that affect names.
#[derive(Clone)]
struct Scope {
    target: Option<String>,
    qualified_elements: bool,
    qualified_attributes: bool,
    base: UriRef,
}

/// Reads schema documents and collects their components.
pub struct Reader<'a> {
    loader: &'a dyn Loader,
    sources: &'a SourceMap,
    limits: Limits,
    loaded: HashSet<String>,
    pub components: Components,
    pub diagnostics: Diagnostics,
}

impl std::fmt::Debug for Reader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader")
            .field("loaded", &self.loaded)
            .finish_non_exhaustive()
    }
}

impl<'a> Reader<'a> {
    pub fn new(loader: &'a dyn Loader, sources: &'a SourceMap, limits: Limits) -> Self {
        Self {
            loader,
            sources,
            limits,
            loaded: HashSet::new(),
            components: Components::default(),
            diagnostics: Diagnostics::new(),
        }
    }

    fn error(&mut self, code: faultline_source::DiagnosticCode, message: String, span: Span) {
        self.diagnostics.push(
            Diagnostic::error(message)
                .with_code(code)
                .with_primary(span, ""),
        );
    }

    /// Reads the XML document at `uri` (loading it and registering it in
    /// the source map) and returns its root element.
    pub fn read_document(&mut self, uri: &UriRef, referenced_at: Option<Span>) -> Option<Element> {
        let loaded = match self.loader.load(uri) {
            Ok(loaded) => loaded,
            Err(e) => {
                let mut diagnostic =
                    Diagnostic::error(format!("cannot load {uri}: {e}")).with_code(codes::LOAD);
                if let Some(span) = referenced_at {
                    diagnostic = diagnostic.with_primary(span, "referenced here");
                }
                self.diagnostics.push(diagnostic);
                return None;
            }
        };
        let file = match self.sources.add(loaded.name, loaded.text) {
            Ok(file) => file,
            Err(e) => {
                self.diagnostics
                    .push(Diagnostic::error(e.to_string()).with_code(codes::LOAD));
                return None;
            }
        };
        if let Err(e) = self.limits.check_file_size(file.len().into()) {
            self.diagnostics
                .push(Diagnostic::error(format!("{uri}: {e}")).with_code(codes::LOAD));
            return None;
        }
        faultline_xml::parse(file.text(), file.id(), &self.limits, &mut self.diagnostics)
            .map(|document| document.root)
    }

    /// Loads the schema document at `uri` and everything it includes and
    /// imports.
    pub fn load(&mut self, uri: &UriRef) {
        self.load_with(uri, Inherit::Own, None);
    }

    fn load_with(&mut self, uri: &UriRef, namespace: Inherit, at: Option<Span>) {
        let key = uri.without_fragment().normalized().to_string();
        if !self.loaded.insert(key) {
            return;
        }
        if self.loaded.len() > MAX_DOCUMENTS {
            if let Some(span) = at {
                self.error(
                    codes::LOAD,
                    format!("more than {MAX_DOCUMENTS} schema documents"),
                    span,
                );
            }
            return;
        }
        let Some(root) = self.read_document(uri, at) else {
            return;
        };
        if !root.name.is(XS, "schema") {
            self.error(
                codes::NOT_A_SCHEMA,
                format!(
                    "{uri} is not an XML Schema: its root is <{}>",
                    root.raw_name
                ),
                root.name_span,
            );
            return;
        }
        self.add_schema_with(&root, uri, namespace);
    }

    /// Adds a schema element that is already parsed, such as one embedded
    /// in a WSDL document, whose references resolve against `base`.
    pub fn add_schema(&mut self, schema: &Element, base: &UriRef) {
        self.add_schema_with(schema, base, Inherit::Own);
    }

    fn add_schema_with(&mut self, schema: &Element, base: &UriRef, namespace: Inherit) {
        let declared = schema.attr("targetNamespace").map(str::to_owned);
        let target = match (declared, namespace) {
            (Some(own), _) => Some(own),
            (None, Inherit::From(includer)) => includer,
            (None, Inherit::Own) => None,
        };
        let scope = Scope {
            target,
            qualified_elements: schema.attr("elementFormDefault") == Some("qualified"),
            qualified_attributes: schema.attr("attributeFormDefault") == Some("qualified"),
            base: base.clone(),
        };
        for child in schema.elements() {
            if child.name.namespace.as_deref() == Some(XS) {
                self.global(child, &scope);
            }
        }
    }

    /// One top-level child of a schema.
    fn global(&mut self, child: &Element, scope: &Scope) {
        match child.name.local.as_str() {
            "include" => self.include(child, scope),
            "import" => self.import(child, scope),
            "redefine" | "override" => {
                let span = child.name_span;
                self.diagnostics.push(
                    Diagnostic::warning(format!(
                        "xs:{} is not supported; the redefined components are compared as originally defined",
                        child.name.local
                    ))
                    .with_code(codes::UNSUPPORTED)
                    .with_primary(span, ""),
                );
                self.include(child, scope);
            }
            "element" => {
                if let Some(decl) = self.element_decl(child, scope, true) {
                    let name = decl.name.clone();
                    if self.components.elements.contains_key(&name) {
                        self.duplicate("element", &name, child.name_span);
                    } else {
                        self.components.element_order.push(name.clone());
                        self.components.elements.insert(name, decl);
                    }
                }
            }
            "simpleType" | "complexType" => {
                let Some(name) = self.global_name(child, scope) else {
                    return;
                };
                let def = if child.name.local == "simpleType" {
                    self.simple_type(child, scope).map(TypeDef::Simple)
                } else {
                    self.complex_type(child, scope)
                        .map(|t| TypeDef::Complex(Box::new(t)))
                };
                if let Some(def) = def {
                    if let std::collections::hash_map::Entry::Vacant(slot) =
                        self.components.types.entry(name.clone())
                    {
                        slot.insert(def);
                    } else {
                        self.duplicate("type", &name, child.name_span);
                    }
                }
            }
            "group" => {
                let Some(name) = self.global_name(child, scope) else {
                    return;
                };
                let Some(model) = child
                    .elements()
                    .find(|e| matches!(e.name.local.as_str(), "sequence" | "choice" | "all"))
                else {
                    return;
                };
                if let Some(particle) = self.particle(model, scope) {
                    self.components.groups.insert(name, particle);
                }
            }
            "attributeGroup" => {
                let Some(name) = self.global_name(child, scope) else {
                    return;
                };
                let (attributes, any) = self.attributes(child, scope);
                self.components
                    .attribute_groups
                    .insert(name, (attributes, any));
            }
            "attribute" => {
                if let Some(name) = self.global_name(child, scope) {
                    let span = child.span;
                    let ty = self.type_of(child, scope);
                    self.components.attributes.insert(
                        name.clone(),
                        AttributeUse::Declared {
                            name,
                            ty,
                            required: false,
                            prohibited: false,
                            default: child.attr("default").map(str::to_owned),
                            fixed: child.attr("fixed").map(str::to_owned),
                            span,
                        },
                    );
                }
            }
            _ => {}
        }
    }

    fn duplicate(&mut self, what: &str, name: &QName, span: Span) {
        self.error(
            codes::DUPLICATE,
            format!("{what} {name} is declared twice"),
            span,
        );
    }

    fn global_name(&mut self, element: &Element, scope: &Scope) -> Option<QName> {
        if let Some(name) = element.attr("name") {
            return Some(QName::new(scope.target.as_deref(), name));
        }
        self.error(
            codes::INVALID,
            format!("a global <{}> needs a name", element.raw_name),
            element.name_span,
        );
        None
    }

    fn location(&mut self, element: &Element, scope: &Scope) -> Option<(UriRef, Span)> {
        let attribute = element.attribute("schemaLocation")?;
        let span = attribute.value_span;
        let Ok(reference) = UriRef::parse(&attribute.value) else {
            self.error(
                codes::LOAD,
                "schemaLocation is not a valid URI reference".into(),
                span,
            );
            return None;
        };
        let is_file = scope
            .base
            .scheme()
            .is_some_and(|s| s.eq_ignore_ascii_case("file"));
        let target = if is_file && !reference.has_scheme() {
            if let Some(target) = scope.base.resolve_strict(&reference) {
                target
            } else {
                self.error(
                    codes::LOAD,
                    "schemaLocation points outside the workspace root".into(),
                    span,
                );
                return None;
            }
        } else {
            scope.base.resolve(&reference)
        };
        Some((target, span))
    }

    fn include(&mut self, element: &Element, scope: &Scope) {
        if let Some((uri, span)) = self.location(element, scope) {
            self.load_with(&uri, Inherit::From(scope.target.clone()), Some(span));
        }
    }

    fn import(&mut self, element: &Element, scope: &Scope) {
        // Without a location the imported namespace must come from another
        // schema of the same set (as in WSDL, which embeds several).
        if let Some((uri, span)) = self.location(element, scope) {
            self.load_with(&uri, Inherit::Own, Some(span));
        }
    }

    /// A `QName`-valued attribute, resolved against the prefixes in scope.
    fn qname(&mut self, element: &Element, attribute: &str) -> Option<(QName, Span)> {
        let attr = element.attribute(attribute)?;
        if let Some(name) = element.resolve_qname(&attr.value) {
            return Some((name, attr.value_span));
        }
        let span = attr.value_span;
        let prefix = attr.value.split(':').next().unwrap_or_default().to_owned();
        self.error(
            codes::UNBOUND_PREFIX,
            format!("namespace prefix `{prefix}` is not declared"),
            span,
        );
        None
    }

    fn documentation(element: &Element) -> Option<String> {
        let annotation = element.child(XS, "annotation")?;
        let text: Vec<String> = annotation
            .children_named(XS, "documentation")
            .map(|d| d.text().trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect();
        (!text.is_empty()).then(|| text.join("\n\n"))
    }

    fn number(&mut self, element: &Element, attribute: &str) -> Option<u64> {
        let attr = element.attribute(attribute)?;
        if let Ok(n) = attr.value.trim().parse() {
            return Some(n);
        }
        let span = attr.value_span;
        self.error(
            codes::INVALID,
            format!("{attribute} must be a non-negative integer"),
            span,
        );
        None
    }

    fn occurs(&mut self, element: &Element) -> Occurs {
        let min = self.number(element, "minOccurs").unwrap_or(1);
        let max = match element.attr("maxOccurs").map(str::trim) {
            Some("unbounded") => None,
            Some(_) => Some(self.number(element, "maxOccurs").unwrap_or(1)),
            None => Some(1),
        };
        Occurs { min, max }
    }

    /// The type an element or attribute declares: its `type` attribute or
    /// an inline definition.
    fn type_of(&mut self, element: &Element, scope: &Scope) -> Option<TypeRef> {
        if let Some((name, span)) = self.qname(element, "type") {
            return Some(TypeRef::Named(name, span));
        }
        if let Some(inline) = element.child(XS, "simpleType") {
            return self
                .simple_type(inline, scope)
                .map(|t| TypeRef::Anonymous(Box::new(TypeDef::Simple(t))));
        }
        if let Some(inline) = element.child(XS, "complexType") {
            return self
                .complex_type(inline, scope)
                .map(|t| TypeRef::Anonymous(Box::new(TypeDef::Complex(Box::new(t)))));
        }
        None
    }

    fn element_decl(
        &mut self,
        element: &Element,
        scope: &Scope,
        global: bool,
    ) -> Option<ElementDecl> {
        let Some(local) = element.attr("name") else {
            self.error(
                codes::INVALID,
                "an element declaration needs a name or a ref".into(),
                element.name_span,
            );
            return None;
        };
        let qualified = global
            || match element.attr("form") {
                Some(form) => form == "qualified",
                None => scope.qualified_elements,
            };
        let name = QName::new(qualified.then_some(()).and(scope.target.as_deref()), local);
        let ty = self.type_of(element, scope);
        let substitution_group = self.qname(element, "substitutionGroup").map(|(q, _)| q);
        Some(ElementDecl {
            name,
            ty,
            nillable: element.attr("nillable") == Some("true"),
            abstract_: element.attr("abstract") == Some("true"),
            default: element.attr("default").map(str::to_owned),
            fixed: element.attr("fixed").map(str::to_owned),
            substitution_group,
            documentation: Self::documentation(element),
            span: element.span,
        })
    }

    /// A particle: an element, a model group, a group reference or `any`.
    fn particle(&mut self, element: &Element, scope: &Scope) -> Option<Particle> {
        let occurs = self.occurs(element);
        let term = match element.name.local.as_str() {
            "element" => {
                if let Some((name, _)) = self.qname(element, "ref") {
                    Term::ElementRef(name)
                } else {
                    Term::Element(self.element_decl(element, scope, false)?)
                }
            }
            "sequence" | "choice" | "all" => {
                let particles: Vec<Particle> = element
                    .elements()
                    .filter(|e| e.name.namespace.as_deref() == Some(XS))
                    .filter(|e| e.name.local != "annotation")
                    .filter_map(|e| self.particle(e, scope))
                    .collect();
                match element.name.local.as_str() {
                    "sequence" => Term::Sequence(particles),
                    "choice" => Term::Choice(particles),
                    _ => Term::All(particles),
                }
            }
            "group" => Term::GroupRef(self.qname(element, "ref")?.0),
            "any" => Term::Any,
            _ => return None,
        };
        Some(Particle {
            term,
            occurs,
            span: element.span,
        })
    }

    fn facets(&mut self, restriction: &Element) -> Facets {
        let mut facets = Facets::default();
        let mut patterns = Vec::new();
        for facet in restriction.elements() {
            let Some(value) = facet.attribute("value") else {
                continue;
            };
            let text = value.value.clone();
            match facet.name.local.as_str() {
                "enumeration" => facets.enumeration.push((text, facet.span)),
                "pattern" => patterns.push(text),
                "length" => facets.length = self.number(facet, "value"),
                "minLength" => facets.min_length = self.number(facet, "value"),
                "maxLength" => facets.max_length = self.number(facet, "value"),
                "minInclusive" => facets.min_inclusive = Some(text),
                "maxInclusive" => facets.max_inclusive = Some(text),
                "minExclusive" => facets.min_exclusive = Some(text),
                "maxExclusive" => facets.max_exclusive = Some(text),
                "totalDigits" => facets.total_digits = self.number(facet, "value"),
                "fractionDigits" => facets.fraction_digits = self.number(facet, "value"),
                _ => {}
            }
        }
        // The patterns of one restriction step are alternatives.
        match patterns.len() {
            0 => {}
            1 => facets.patterns = patterns,
            _ => facets.patterns = vec![patterns.join("|")],
        }
        facets
    }

    /// The base of a restriction or extension: its `base` attribute, or an
    /// inline simple type.
    fn base(&mut self, derivation: &Element, scope: &Scope) -> Option<TypeRef> {
        if let Some((name, span)) = self.qname(derivation, "base") {
            return Some(TypeRef::Named(name, span));
        }
        let inline = derivation.child(XS, "simpleType")?;
        self.simple_type(inline, scope)
            .map(|t| TypeRef::Anonymous(Box::new(TypeDef::Simple(t))))
    }

    fn simple_type(&mut self, element: &Element, scope: &Scope) -> Option<SimpleType> {
        let name = element
            .attr("name")
            .map(|n| QName::new(scope.target.as_deref(), n));
        let variety = if let Some(restriction) = element.child(XS, "restriction") {
            let base = self.base(restriction, scope).unwrap_or_else(|| {
                TypeRef::Named(QName::new(Some(XS), "anySimpleType"), restriction.span)
            });
            SimpleVariety::Restriction {
                base,
                facets: Box::new(self.facets(restriction)),
            }
        } else if let Some(list) = element.child(XS, "list") {
            let item = match self.qname(list, "itemType") {
                Some((name, span)) => TypeRef::Named(name, span),
                None => match list.child(XS, "simpleType") {
                    Some(inline) => TypeRef::Anonymous(Box::new(TypeDef::Simple(
                        self.simple_type(inline, scope)?,
                    ))),
                    None => TypeRef::Named(QName::new(Some(XS), "anySimpleType"), list.span),
                },
            };
            SimpleVariety::List { item }
        } else if let Some(union) = element.child(XS, "union") {
            let mut members = Vec::new();
            if let Some(attr) = union.attribute("memberTypes") {
                for text in attr.value.split_whitespace() {
                    match union.resolve_qname(text) {
                        Some(name) => members.push(TypeRef::Named(name, attr.value_span)),
                        None => self.error(
                            codes::UNBOUND_PREFIX,
                            format!("the prefix of {text} is not declared"),
                            attr.value_span,
                        ),
                    }
                }
            }
            for inline in union.children_named(XS, "simpleType") {
                if let Some(t) = self.simple_type(inline, scope) {
                    members.push(TypeRef::Anonymous(Box::new(TypeDef::Simple(t))));
                }
            }
            SimpleVariety::Union { members }
        } else {
            self.error(
                codes::INVALID,
                "a simple type needs a restriction, list or union".into(),
                element.name_span,
            );
            return None;
        };
        Some(SimpleType {
            name,
            variety,
            documentation: Self::documentation(element),
            span: element.span,
        })
    }

    fn complex_type(&mut self, element: &Element, scope: &Scope) -> Option<ComplexType> {
        let name = element
            .attr("name")
            .map(|n| QName::new(scope.target.as_deref(), n));
        let mut mixed = element.attr("mixed") == Some("true");
        let mut base = None;
        let content;
        let (attributes, any_attribute);
        if let Some(simple) = element.child(XS, "simpleContent") {
            let derivation = simple
                .elements()
                .find(|e| matches!(e.name.local.as_str(), "extension" | "restriction"))?;
            let kind = derivation_kind(derivation);
            let base_type = self.base(derivation, scope)?;
            if let TypeRef::Named(q, span) = &base_type {
                base = Some((kind, q.clone(), *span));
            }
            content = ContentModel::Simple {
                base: base_type,
                facets: self.facets(derivation),
            };
            (attributes, any_attribute) = self.attributes(derivation, scope);
        } else if let Some(complex) = element.child(XS, "complexContent") {
            mixed |= complex.attr("mixed") == Some("true");
            let derivation = complex
                .elements()
                .find(|e| matches!(e.name.local.as_str(), "extension" | "restriction"))?;
            if let Some((q, span)) = self.qname(derivation, "base") {
                base = Some((derivation_kind(derivation), q, span));
            }
            content = self.model_of(derivation, scope);
            (attributes, any_attribute) = self.attributes(derivation, scope);
        } else {
            content = self.model_of(element, scope);
            (attributes, any_attribute) = self.attributes(element, scope);
        }
        Some(ComplexType {
            name,
            base,
            content,
            attributes,
            any_attribute,
            mixed,
            abstract_: element.attr("abstract") == Some("true"),
            documentation: Self::documentation(element),
            span: element.span,
        })
    }

    /// The particle directly inside `element`, if any.
    fn model_of(&mut self, element: &Element, scope: &Scope) -> ContentModel {
        let model = element.elements().find(|e| {
            matches!(
                e.name.local.as_str(),
                "sequence" | "choice" | "all" | "group"
            )
        });
        match model.and_then(|m| self.particle(m, scope)) {
            Some(particle) => ContentModel::Elements(particle),
            None => ContentModel::Empty,
        }
    }

    /// The attribute uses directly inside `element`, and whether it allows
    /// other attributes.
    fn attributes(&mut self, element: &Element, scope: &Scope) -> (Vec<AttributeUse>, bool) {
        let mut out = Vec::new();
        let mut any = false;
        for child in element.elements() {
            match child.name.local.as_str() {
                "attribute" => {
                    let use_ = child.attr("use").unwrap_or("optional");
                    let required = use_ == "required";
                    let prohibited = use_ == "prohibited";
                    if let Some((name, _)) = self.qname(child, "ref") {
                        out.push(AttributeUse::Ref {
                            name,
                            required,
                            prohibited,
                            span: child.span,
                        });
                        continue;
                    }
                    let Some(local) = child.attr("name") else {
                        self.error(
                            codes::INVALID,
                            "an attribute needs a name or a ref".into(),
                            child.name_span,
                        );
                        continue;
                    };
                    let qualified = match child.attr("form") {
                        Some(form) => form == "qualified",
                        None => scope.qualified_attributes,
                    };
                    let name =
                        QName::new(qualified.then_some(()).and(scope.target.as_deref()), local);
                    let ty = self.type_of(child, scope);
                    out.push(AttributeUse::Declared {
                        name,
                        ty,
                        required,
                        prohibited,
                        default: child.attr("default").map(str::to_owned),
                        fixed: child.attr("fixed").map(str::to_owned),
                        span: child.span,
                    });
                }
                "attributeGroup" => {
                    if let Some((name, span)) = self.qname(child, "ref") {
                        out.push(AttributeUse::GroupRef(name, span));
                    }
                }
                "anyAttribute" => any = true,
                _ => {}
            }
        }
        (out, any)
    }
}

fn derivation_kind(element: &Element) -> Derivation {
    if element.name.local == "extension" {
        Derivation::Extension
    } else {
        Derivation::Restriction
    }
}

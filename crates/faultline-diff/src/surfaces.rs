//! Comparison of API surfaces: operations, parameters, bodies, responses,
//! callbacks and standalone schemas. Paths match those of
//! [`Api::roots`](faultline_ir::Api::roots).

use faultline_ir::{
    Content, IrPath, Operation, Parameter, Polarity, RequestBody, Response, Role,
    SecurityRequirement, Segment, Serialization, TypeId,
};

use crate::change::ChangeKind;
use crate::differ::Differ;

impl<'a> Differ<'a> {
    pub(crate) fn operations(
        &mut self,
        old: &'a [Operation],
        new: &'a [Operation],
        prefix: &IrPath,
        callback: bool,
    ) {
        let (request, response) = if callback {
            (Polarity::OUT, Polarity::IN)
        } else {
            (Polarity::IN, Polarity::OUT)
        };
        let added: Vec<&'a Operation> = new
            .iter()
            .filter(|op| !old.iter().any(|o| o.key.identity() == op.key.identity()))
            .collect();
        let mut moved_to: Vec<bool> = vec![false; added.len()];
        for op in old {
            let path = prefix.join(Segment::Operation(op.key.identity()));
            if let Some(counterpart) = new.iter().find(|n| n.key.identity() == op.key.identity()) {
                self.operation(op, counterpart, &path, request, response, callback);
                continue;
            }
            // An operation removed and one added with the same shape is one
            // operation that moved.
            let destination = added.iter().enumerate().position(|(i, candidate)| {
                !moved_to[i] && self.same_shape(op, candidate, request, response, callback)
            });
            match destination {
                Some(i) => {
                    moved_to[i] = true;
                    let kind = ChangeKind::OperationMoved {
                        to: added[i].key.identity(),
                    };
                    self.surface(kind, path, request, (op.origin, added[i].origin));
                }
                None => self.surface(
                    ChangeKind::OperationRemoved,
                    path,
                    request,
                    (op.origin, None),
                ),
            }
        }
        for (op, moved) in added.iter().zip(moved_to) {
            if !moved {
                let path = prefix.join(Segment::Operation(op.key.identity()));
                self.surface(ChangeKind::OperationAdded, path, request, (None, op.origin));
            }
        }
    }

    /// Whether two operations with different keys differ in nothing but
    /// their key (and operation id): the same method, parameters, bodies
    /// and responses.
    fn same_shape(
        &self,
        old: &'a Operation,
        new: &'a Operation,
        request: Polarity,
        response: Polarity,
        callback: bool,
    ) -> bool {
        use faultline_ir::OperationKey;
        let same_kind = match (&old.key, &new.key) {
            (OperationKey::Http { method: a, .. }, OperationKey::Http { method: b, .. }) => a == b,
            (OperationKey::Rpc { .. }, OperationKey::Rpc { .. }) => true,
            _ => false,
        };
        let signature = |op: &Operation| {
            (
                op.parameters.len(),
                op.request_body.as_ref().map(|b| b.content.len()),
                op.responses
                    .iter()
                    .map(|r| r.status.to_string())
                    .collect::<Vec<_>>(),
                op.callbacks.len(),
            )
        };
        if !same_kind || signature(old) != signature(new) {
            return false;
        }
        let mut probe = Differ::new(self.old, self.new);
        probe.operation(old, new, &IrPath::new(), request, response, callback);
        probe
            .changes
            .iter()
            .all(|c| matches!(c.kind, ChangeKind::OperationIdChanged { .. }))
    }

    fn operation(
        &mut self,
        old: &'a Operation,
        new: &'a Operation,
        path: &IrPath,
        request: Polarity,
        response: Polarity,
        callback: bool,
    ) {
        let spans = (old.origin, new.origin);
        if old.operation_id != new.operation_id {
            let kind = ChangeKind::OperationIdChanged {
                old: old.operation_id.clone(),
                new: new.operation_id.clone(),
            };
            self.surface(kind, path.clone(), request, spans);
        }
        if old.meta.deprecated != new.meta.deprecated {
            let kind = ChangeKind::DeprecatedChanged {
                deprecated: new.meta.deprecated,
            };
            self.surface(kind, path.clone(), request, spans);
        }
        self.security(
            old.security.as_deref(),
            new.security.as_deref(),
            path,
            request,
        );
        let (a, b) = (
            old.streaming.unwrap_or_default(),
            new.streaming.unwrap_or_default(),
        );
        for (side, was, is, polarity) in [
            ("request", a.request, b.request, request),
            ("response", a.response, b.response, response),
        ] {
            if was != is {
                let kind = ChangeKind::StreamingChanged {
                    side: side.to_owned(),
                    streaming: is,
                };
                self.surface(kind, path.clone(), polarity, spans);
            }
        }
        self.parameters(&old.parameters, &new.parameters, path, request);
        self.request_bodies(
            old.request_body.as_ref(),
            new.request_body.as_ref(),
            path,
            request,
        );
        self.responses(&old.responses, &new.responses, path, response);

        for cb in &old.callbacks {
            let at = path.join(Segment::Callback(cb.name.clone()));
            match new.callbacks.iter().find(|c| c.name == cb.name) {
                Some(counterpart) => {
                    self.operations(&cb.operations, &counterpart.operations, &at, !callback);
                }
                None => self.surface(
                    ChangeKind::CallbackRemoved {
                        name: cb.name.clone(),
                    },
                    at,
                    request,
                    (cb.origin, None),
                ),
            }
        }
        for cb in &new.callbacks {
            if !old.callbacks.iter().any(|c| c.name == cb.name) {
                let at = path.join(Segment::Callback(cb.name.clone()));
                self.surface(
                    ChangeKind::CallbackAdded {
                        name: cb.name.clone(),
                    },
                    at,
                    request,
                    (None, cb.origin),
                );
            }
        }
    }

    fn parameters(
        &mut self,
        old: &'a [Parameter],
        new: &'a [Parameter],
        path: &IrPath,
        polarity: Polarity,
    ) {
        let key = |p: &Parameter| (p.location, p.identity_name());
        for p in old {
            let at = path.join(Segment::Parameter(p.location, p.identity_name()));
            if let Some(n) = new.iter().find(|n| key(n) == key(p)) {
                if p.required != n.required {
                    let kind = ChangeKind::ParameterRequiredChanged {
                        location: p.location,
                        name: p.name.clone(),
                        required: n.required,
                    };
                    self.surface(kind, at.clone(), polarity, (p.origin, n.origin));
                }
                if let (Some(a), Some(b)) = (&p.serialization, &n.serialization)
                    && a != b
                {
                    let kind = ChangeKind::ParameterSerializationChanged {
                        location: p.location,
                        name: p.name.clone(),
                        old: describe_serialization(a),
                        new: describe_serialization(b),
                    };
                    self.surface(kind, at.clone(), polarity, (p.origin, n.origin));
                }
                self.compare(p.ty, n.ty, &at, polarity);
            } else {
                let kind = ChangeKind::ParameterRemoved {
                    location: p.location,
                    name: p.name.clone(),
                    required: p.required,
                };
                self.surface(kind, at, polarity, (p.origin, None));
            }
        }
        for n in new {
            if !old.iter().any(|p| key(p) == key(n)) {
                let kind = ChangeKind::ParameterAdded {
                    location: n.location,
                    name: n.name.clone(),
                    required: n.required,
                };
                let at = path.join(Segment::Parameter(n.location, n.identity_name()));
                self.surface(kind, at, polarity, (None, n.origin));
            }
        }
    }

    /// Compares security alternatives as sets. No requirement at all is the
    /// single anonymous alternative, so "authentication became required"
    /// appears as the anonymous alternative being removed.
    fn security(
        &mut self,
        old: Option<&[SecurityRequirement]>,
        new: Option<&[SecurityRequirement]>,
        path: &IrPath,
        polarity: Polarity,
    ) {
        let (Some(old), Some(new)) = (old, new) else {
            return;
        };
        let effective = |list: &[SecurityRequirement]| -> Vec<String> {
            if list.is_empty() {
                vec![SecurityRequirement::default().to_string()]
            } else {
                list.iter().map(ToString::to_string).collect()
            }
        };
        let (old, new) = (effective(old), effective(new));
        for requirement in old.iter().filter(|r| !new.contains(r)) {
            let kind = ChangeKind::SecurityRequirementRemoved {
                requirement: requirement.clone(),
            };
            self.surface(kind, path.clone(), polarity, (None, None));
        }
        for requirement in new.iter().filter(|r| !old.contains(r)) {
            let kind = ChangeKind::SecurityRequirementAdded {
                requirement: requirement.clone(),
            };
            self.surface(kind, path.clone(), polarity, (None, None));
        }
    }

    fn request_bodies(
        &mut self,
        old: Option<&'a RequestBody>,
        new: Option<&'a RequestBody>,
        path: &IrPath,
        polarity: Polarity,
    ) {
        match (old, new) {
            (None, None) => {}
            (Some(o), None) => self.surface(
                ChangeKind::RequestBodyRemoved,
                path.clone(),
                polarity,
                (o.origin, None),
            ),
            (None, Some(n)) => self.surface(
                ChangeKind::RequestBodyAdded {
                    required: n.required,
                },
                path.clone(),
                polarity,
                (None, n.origin),
            ),
            (Some(o), Some(n)) => {
                if o.required != n.required {
                    self.surface(
                        ChangeKind::RequestBodyRequiredChanged {
                            required: n.required,
                        },
                        path.clone(),
                        polarity,
                        (o.origin, n.origin),
                    );
                }
                self.contents(&o.content, &n.content, path, polarity, Segment::RequestBody);
            }
        }
    }

    fn responses(
        &mut self,
        old: &'a [Response],
        new: &'a [Response],
        path: &IrPath,
        polarity: Polarity,
    ) {
        for r in old {
            let at = path.join(Segment::Response(r.status.to_string()));
            let Some(n) = new.iter().find(|n| n.status == r.status) else {
                self.surface(
                    ChangeKind::ResponseRemoved { status: r.status },
                    at,
                    polarity,
                    (r.origin, None),
                );
                continue;
            };
            for h in &r.headers {
                let name = h.name.to_ascii_lowercase();
                let header_path = at.join(Segment::Header(name.clone()));
                if let Some(nh) = n
                    .headers
                    .iter()
                    .find(|x| x.name.eq_ignore_ascii_case(&h.name))
                {
                    if h.required != nh.required {
                        let kind = ChangeKind::ResponseHeaderRequiredChanged {
                            name: h.name.clone(),
                            required: nh.required,
                        };
                        self.surface(kind, header_path.clone(), polarity, (h.origin, nh.origin));
                    }
                    self.compare(h.ty, nh.ty, &header_path, polarity);
                } else {
                    let kind = ChangeKind::ResponseHeaderRemoved {
                        name: h.name.clone(),
                        required: h.required,
                    };
                    self.surface(kind, header_path, polarity, (h.origin, None));
                }
            }
            for nh in &n.headers {
                if !r
                    .headers
                    .iter()
                    .any(|h| h.name.eq_ignore_ascii_case(&nh.name))
                {
                    let kind = ChangeKind::ResponseHeaderAdded {
                        name: nh.name.clone(),
                        required: nh.required,
                    };
                    let header_path = at.join(Segment::Header(nh.name.to_ascii_lowercase()));
                    self.surface(kind, header_path, polarity, (None, nh.origin));
                }
            }
            self.contents(&r.content, &n.content, &at, polarity, Segment::Body);
        }
        for n in new {
            if !old.iter().any(|r| r.status == n.status) {
                let at = path.join(Segment::Response(n.status.to_string()));
                self.surface(
                    ChangeKind::ResponseAdded { status: n.status },
                    at,
                    polarity,
                    (None, n.origin),
                );
            }
        }
    }

    fn contents(
        &mut self,
        old: &'a [Content],
        new: &'a [Content],
        path: &IrPath,
        polarity: Polarity,
        segment: fn(String) -> Segment,
    ) {
        for c in old {
            let at = path.join(segment(c.media_type.clone()));
            match new.iter().find(|n| n.media_type == c.media_type) {
                Some(n) => self.compare(c.ty, n.ty, &at, polarity),
                None => self.surface(
                    ChangeKind::MediaTypeRemoved {
                        media_type: c.media_type.clone(),
                    },
                    at,
                    polarity,
                    (c.origin, None),
                ),
            }
        }
        for n in new {
            if !old.iter().any(|c| c.media_type == n.media_type) {
                let at = path.join(segment(n.media_type.clone()));
                let kind = ChangeKind::MediaTypeAdded {
                    media_type: n.media_type.clone(),
                };
                self.surface(kind, at, polarity, (None, n.origin));
            }
        }
    }

    pub(crate) fn webhooks(&mut self) {
        let (old, new) = (self.old, self.new);
        for hook in &old.webhooks {
            let at = IrPath::new().join(Segment::Webhook(hook.name.clone()));
            if let Some(counterpart) = new.webhooks.iter().find(|h| h.name == hook.name) {
                self.operations(&hook.operations, &counterpart.operations, &at, true);
            } else {
                let kind = ChangeKind::WebhookRemoved {
                    name: hook.name.clone(),
                };
                self.surface(kind, at, Polarity::OUT, (hook.origin, None));
            }
        }
        for hook in &new.webhooks {
            if !old.webhooks.iter().any(|h| h.name == hook.name) {
                let at = IrPath::new().join(Segment::Webhook(hook.name.clone()));
                let kind = ChangeKind::WebhookAdded {
                    name: hook.name.clone(),
                };
                self.surface(kind, at, Polarity::OUT, (None, hook.origin));
            }
        }
    }

    pub(crate) fn schemas(&mut self) {
        let (old, new) = (self.old, self.new);
        for s in &old.schemas {
            let path = IrPath::new().join(Segment::Schema(s.name.clone()));
            let polarity = role_polarity(s.role);
            match new.schemas.iter().find(|n| n.name == s.name) {
                Some(n) => self.compare(s.ty, n.ty, &path, polarity.union(role_polarity(n.role))),
                None => self.surface(
                    ChangeKind::SchemaRemoved {
                        name: s.name.clone(),
                    },
                    path,
                    polarity,
                    (s.origin, None),
                ),
            }
        }
        for n in &new.schemas {
            if !old.schemas.iter().any(|s| s.name == n.name) {
                let path = IrPath::new().join(Segment::Schema(n.name.clone()));
                let kind = ChangeKind::SchemaAdded {
                    name: n.name.clone(),
                };
                self.surface(kind, path, role_polarity(n.role), (None, n.origin));
            }
        }
    }

    /// Named types not reachable from any root are compared on their own,
    /// with no polarity: they may be a library of definitions used elsewhere.
    pub(crate) fn unreachable_definitions(&mut self) {
        let (old, new) = (self.old, self.new);
        let pending: Vec<(String, TypeId, TypeId)> = old
            .named
            .iter()
            .filter(|&(_, &ty)| !self.visited_old.contains(&old.types.resolve(ty)))
            .filter_map(|(name, &ty)| new.named.get(name).map(|&n| (name.to_string(), ty, n)))
            .collect();
        for (name, o, n) in pending {
            let path = IrPath::new().join(Segment::Type(name));
            self.compare(o, n, &path, Polarity::NONE);
        }
    }
}

fn describe_serialization(s: &Serialization) -> String {
    let mut out = format!("style={} explode={}", s.style, s.explode);
    if s.allow_reserved {
        out.push_str(" allowReserved");
    }
    if s.allow_empty_value {
        out.push_str(" allowEmptyValue");
    }
    out
}

fn role_polarity(role: Role) -> Polarity {
    match role {
        Role::Input => Polarity::IN,
        Role::Output => Polarity::OUT,
        Role::Both => Polarity::BOTH,
    }
}

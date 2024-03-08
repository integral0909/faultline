use std::fmt;

/// A JSON Schema dialect: the specification version whose keyword semantics
/// apply to a schema resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dialect {
    Draft4,
    Draft6,
    Draft7,
    Draft2019_09,
    Draft2020_12,
}

impl Dialect {
    /// Identifies a dialect from a `$schema` value. Accepts `http` and
    /// `https`, with or without the trailing empty fragment, and the OpenAPI
    /// 3.1 base dialect, which is 2020-12 with extra annotation keywords.
    pub fn from_meta_schema(uri: &str) -> Option<Dialect> {
        let trimmed = uri.trim_end_matches('#');
        let without_scheme = trimmed
            .strip_prefix("https://")
            .or_else(|| trimmed.strip_prefix("http://"))?;
        Some(match without_scheme {
            "json-schema.org/draft-04/schema" => Dialect::Draft4,
            "json-schema.org/draft-06/schema" => Dialect::Draft6,
            "json-schema.org/draft-07/schema" => Dialect::Draft7,
            "json-schema.org/draft/2019-09/schema" => Dialect::Draft2019_09,
            "json-schema.org/draft/2020-12/schema" | "spec.openapis.org/oas/3.1/dialect/base" => {
                Dialect::Draft2020_12
            }
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Dialect::Draft4 => "draft-04",
            Dialect::Draft6 => "draft-06",
            Dialect::Draft7 => "draft-07",
            Dialect::Draft2019_09 => "2019-09",
            Dialect::Draft2020_12 => "2020-12",
        }
    }

    /// The keyword that declares a resource identifier.
    pub fn id_keyword(self) -> &'static str {
        if self == Dialect::Draft4 { "id" } else { "$id" }
    }

    /// Up to draft-07, `$ref` replaces the whole schema object and sibling
    /// keywords are ignored. From 2019-09 it is an applicator like `allOf`.
    pub fn ref_overrides_siblings(self) -> bool {
        self <= Dialect::Draft7
    }

    /// `$anchor` exists; before 2019-09, anchors were fragment-only `$id`s.
    pub fn has_anchor_keyword(self) -> bool {
        self >= Dialect::Draft2019_09
    }

    /// Draft-04 spells exclusive bounds as booleans modifying
    /// `minimum`/`maximum`; later drafts give them their own numeric values.
    pub fn boolean_exclusive_bounds(self) -> bool {
        self == Dialect::Draft4
    }

    /// `true` and `false` are valid schemas from draft-06 on.
    pub fn has_boolean_schemas(self) -> bool {
        self >= Dialect::Draft6
    }

    /// `const`, `contains`, `propertyNames` and numeric exclusive bounds.
    pub fn has_draft6_keywords(self) -> bool {
        self >= Dialect::Draft6
    }

    /// `if`/`then`/`else`.
    pub fn has_conditionals(self) -> bool {
        self >= Dialect::Draft7
    }

    /// `dependentRequired`/`dependentSchemas`, `minContains`/`maxContains`,
    /// `unevaluated*`, `$defs`. Earlier drafts use `dependencies`.
    pub fn has_2019_keywords(self) -> bool {
        self >= Dialect::Draft2019_09
    }

    /// `prefixItems` for tuples, with `items` for the rest. Earlier drafts
    /// use an array-valued `items` with `additionalItems`.
    pub fn has_prefix_items(self) -> bool {
        self >= Dialect::Draft2020_12
    }

    /// `$recursiveRef`/`$recursiveAnchor` (2019-09 only).
    pub fn has_recursive_ref(self) -> bool {
        self == Dialect::Draft2019_09
    }

    /// `$dynamicRef`/`$dynamicAnchor` (2020-12).
    pub fn has_dynamic_ref(self) -> bool {
        self >= Dialect::Draft2020_12
    }
}

impl fmt::Display for Dialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_schema_uris_in_common_spellings() {
        let cases = [
            ("http://json-schema.org/draft-04/schema#", Dialect::Draft4),
            ("https://json-schema.org/draft-06/schema", Dialect::Draft6),
            ("http://json-schema.org/draft-07/schema#", Dialect::Draft7),
            (
                "https://json-schema.org/draft/2019-09/schema",
                Dialect::Draft2019_09,
            ),
            (
                "https://json-schema.org/draft/2020-12/schema",
                Dialect::Draft2020_12,
            ),
            (
                "https://spec.openapis.org/oas/3.1/dialect/base",
                Dialect::Draft2020_12,
            ),
        ];
        for (uri, dialect) in cases {
            assert_eq!(Dialect::from_meta_schema(uri), Some(dialect), "{uri}");
        }
        assert_eq!(
            Dialect::from_meta_schema("https://example.com/my-dialect"),
            None
        );
        assert_eq!(Dialect::from_meta_schema("draft-07"), None);
    }

    #[test]
    fn feature_boundaries() {
        assert_eq!(Dialect::Draft4.id_keyword(), "id");
        assert_eq!(Dialect::Draft6.id_keyword(), "$id");
        assert!(Dialect::Draft7.ref_overrides_siblings());
        assert!(!Dialect::Draft2019_09.ref_overrides_siblings());
        assert!(Dialect::Draft2019_09.has_recursive_ref());
        assert!(!Dialect::Draft2020_12.has_recursive_ref());
        assert!(Dialect::Draft2020_12.has_dynamic_ref());
    }
}

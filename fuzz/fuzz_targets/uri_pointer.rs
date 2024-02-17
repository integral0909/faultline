//! Exercises URI reference resolution and JSON Pointer parsing with
//! arbitrary strings, checking round trips and normalization idempotence.
#![no_main]

use faultline_doc::{JsonPointer, UriRef};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (&str, &str)| {
    let (base, reference) = input;
    if let (Ok(base), Ok(reference)) = (UriRef::parse(base), UriRef::parse(reference)) {
        let resolved = base.resolve(&reference);
        let normalized = resolved.normalized();
        assert_eq!(normalized.normalized(), normalized, "normalization must be idempotent");
    }
    if let Ok(pointer) = JsonPointer::parse(reference) {
        assert_eq!(JsonPointer::parse(&pointer.to_string()).unwrap(), pointer);
        assert_eq!(JsonPointer::from_uri_fragment(&pointer.to_uri_fragment()).unwrap(), pointer);
    }
});

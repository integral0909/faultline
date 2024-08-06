//! Parses arbitrary bytes as a registry request: parsing must never panic,
//! must stay within its limits, and whatever it accepts must be well
//! formed. Bodies that are JSON also go through bundle validation, whose
//! accepted paths must never escape their bundle.
#![no_main]

use faultline_registry::http::{Options, read_request};
use faultline_registry::store::{Bundle, validate_path};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let options = Options {
        max_body_bytes: 4096,
        max_header_bytes: 1024,
        ..Options::default()
    };
    let mut reader = data;
    let mut interim = Vec::new();
    let Ok(request) = read_request(&mut reader, &mut interim, &options) else {
        return;
    };
    assert!(request.body.len() <= options.max_body_bytes);
    assert!(!request.method.is_empty());
    assert!(request.method.bytes().all(|b| b.is_ascii_uppercase()));
    for segment in &request.segments {
        assert!(!segment.is_empty());
        assert!(!segment.chars().any(char::is_control));
    }
    for name in request.headers.keys() {
        assert_eq!(name, &name.to_ascii_lowercase());
    }
    if let Ok(value) = serde_json::from_slice(&request.body)
        && let Ok(bundle) = Bundle::from_json(&value)
    {
        assert!(bundle.files.contains_key(&bundle.entry));
        for path in bundle.files.keys() {
            assert!(validate_path(path).is_ok());
            assert!(!path.starts_with('/'));
            assert!(!path.split('/').any(|s| s == ".." || s == "."));
        }
    }
});

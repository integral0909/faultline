//! Compiles arbitrary patterns: compilation must never panic or exceed its
//! limits, matching must terminate, a pattern never differs from itself,
//! and samples and differences must be members of the right languages.
#![no_main]

use faultline_regex::{Regex, difference};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (&str, &str, &str)| {
    let (pattern, other, text) = input;
    let Ok(regex) = Regex::new(pattern) else {
        return;
    };
    let _ = regex.is_match(text);
    assert_eq!(difference(&regex, &regex, 0, None), None);
    for sample in regex.samples(3, 0, Some(16)) {
        assert!(regex.is_match(&sample), "{pattern:?} rejects its sample {sample:?}");
    }
    if let Ok(other) = Regex::new(other)
        && let Some(witness) = difference(&regex, &other, 0, None)
    {
        assert!(regex.is_match(&witness));
        assert!(!other.is_match(&witness));
    }
});

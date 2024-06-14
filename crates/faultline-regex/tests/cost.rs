//! Worst cases stay bounded: large quantifiers and nested repetition either
//! compile quickly or are refused.

use std::time::{Duration, Instant};

use faultline_regex::{Regex, difference};

fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let out = f();
    (out, start.elapsed())
}

#[test]
fn large_patterns_are_bounded() {
    for pattern in [
        "a{1000}",
        "(a{100}){10}",
        "[a-z]{0,1000}",
        "(a|b|c|d|e){50}x",
        "(.*a){20}",
        "((a+)+)+b",
        "[\\s\\S]{500}[0-9]{500}",
        "(?:(?:(?:a{9}){9}){9})",
    ] {
        let (result, elapsed) = timed(|| Regex::new(pattern));
        assert!(
            elapsed < Duration::from_secs(2),
            "{pattern} took {elapsed:?} ({:?})",
            result.as_ref().err()
        );
        if let Ok(regex) = result {
            let (_, elapsed) = timed(|| difference(&regex, &Regex::any(), 0, None));
            assert!(
                elapsed < Duration::from_secs(2),
                "{pattern}: difference took {elapsed:?}"
            );
        }
    }
}

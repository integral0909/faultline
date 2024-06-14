//! Compiled `pattern`s, cached per thread.
//!
//! Patterns are compiled on first use and kept for the thread, since the
//! same few patterns are matched against many candidate values during one
//! comparison. Patterns that cannot be analysed (backreferences, lookaround,
//! oversized automata) are remembered as such.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use faultline_regex::Regex;

/// Cached patterns per thread before the cache is cleared.
const CAPACITY: usize = 512;

thread_local! {
    static CACHE: RefCell<HashMap<String, Option<Rc<Regex>>>> = RefCell::new(HashMap::new());
}

/// The compiled form of `pattern`, or `None` if it cannot be analysed.
pub(crate) fn compiled(pattern: &str) -> Option<Rc<Regex>> {
    CACHE.with(|cache| {
        if let Some(found) = cache.borrow().get(pattern) {
            return found.clone();
        }
        let regex = Regex::new(pattern).ok().map(Rc::new);
        let mut cache = cache.borrow_mut();
        if cache.len() >= CAPACITY {
            cache.clear();
        }
        cache.insert(pattern.to_owned(), regex.clone());
        regex
    })
}

/// The language of an optional pattern: every string when absent.
pub(crate) fn language(pattern: Option<&String>) -> Option<Rc<Regex>> {
    match pattern {
        Some(p) => compiled(p),
        None => compiled(""),
    }
}

//! Fault points for the crash tests (`tests/crash.rs`): a test aborts the
//! writing process at a named point to show that a write is all or nothing
//! across a real process death. Compiled to nothing without the
//! `fault-injection` feature, which only this crate's own tests enable.

/// Aborts the process when `GIT_EVIDENCE_FAULT` is `<name>@<n>` and this is the
/// `n`th time `name` is reached (`<name>` alone means the first).
#[cfg(feature = "fault-injection")]
pub fn point(name: &str) {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    static HITS: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());
    let Ok(fault) = std::env::var("GIT_EVIDENCE_FAULT") else {
        return;
    };
    let (target, nth) = fault
        .split_once('@')
        .map_or((fault.as_str(), 1), |(t, n)| (t, n.parse().unwrap_or(1)));
    if target != name {
        return;
    }
    let mut hits = HITS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let count = hits.entry(name.to_owned()).or_insert(0);
    *count += 1;
    if *count == nth {
        std::process::abort();
    }
}

#[cfg(not(feature = "fault-injection"))]
#[inline(always)]
pub const fn point(_name: &str) {}

//! Bounded trace buffer (LANG-C-017).
//!
//! Tracing is opt-in and drained by the caller, but "should drain" is a
//! performance hint: a host that enables tracing and never drains must not
//! grow memory without limit.

use active_core::trace::{set_tracing, take_trail, MAX_TRACE_EVENTS};

#[test]
fn an_undrained_trace_buffer_drops_oldest_events_and_stays_bounded() {
    set_tracing(true);
    let module = active_core::parse("a = 1\n", "trace-cap").expect("parse");
    let engine = active_core::Engine::hermetic();
    // Each evaluation records at least one event; record many more than the
    // cap without draining.
    for _ in 0..(MAX_TRACE_EVENTS + 500) {
        let _ = engine.evaluate(&module);
    }
    let trail = take_trail();
    assert!(
        trail.len() <= MAX_TRACE_EVENTS,
        "buffer must stay bounded, got {}",
        trail.len()
    );
    assert!(
        trail.len() >= MAX_TRACE_EVENTS / 2,
        "drop-oldest keeps the newest window, got {}",
        trail.len()
    );
    set_tracing(false);
}

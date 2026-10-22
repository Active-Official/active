//! Opt-in evaluation tracing (CHECKLIST 3.005, PROPOSAL section 5.2).
//!
//! Disabled by default; enable per thread with [`set_tracing`]. Events
//! carry counts and diagnostic CODES ONLY. Values, sources, paths,
//! spans, and secrets never enter an event by construction: there is
//! no field for them, and the recorder only reads `Diagnostic.code`.
use std::cell::{Cell, RefCell};

use crate::diag::Diagnostic;
use crate::ir::Value;

/// One traced phase of a single evaluation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TraceEvent {
    pub phase: &'static str,
    pub nodes: u64,
    pub diags: u64,
    pub codes: Vec<String>,
}

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static EVENTS: RefCell<Vec<TraceEvent>> = const { RefCell::new(Vec::new()) };
}

/// Turn tracing on or off for this thread. Turning it off discards any
/// buffered events.
pub fn set_tracing(on: bool) {
    ENABLED.with(|e| e.set(on));
    if !on {
        EVENTS.with(|v| v.borrow_mut().clear());
    }
}

/// Drain the buffered events for this thread.
pub fn take_trail() -> Vec<TraceEvent> {
    EVENTS.with(|v| std::mem::take(&mut *v.borrow_mut()))
}

pub(crate) fn record(phase: &'static str, nodes: u64, diags: &[Diagnostic]) {
    if !ENABLED.with(|e| e.get()) {
        return;
    }
    let mut codes: Vec<String> = diags.iter().map(|d| d.code.clone()).collect();
    codes.sort();
    codes.dedup();
    EVENTS.with(|v| {
        v.borrow_mut().push(TraceEvent {
            phase,
            nodes,
            diags: diags.len() as u64,
            codes,
        })
    });
}

/// Iterative result-tree node count (never recursion: depth is bounded
/// by budgets, but the counter must not add its own stack risk).
pub(crate) fn count_nodes(value: &Value) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![value];
    while let Some(v) = stack.pop() {
        total += 1;
        match v {
            Value::Table(map) => stack.extend(map.values()),
            Value::Array(items) => stack.extend(items.iter()),
            _ => {}
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn codes_of(source: &str) -> (Vec<TraceEvent>, Vec<String>) {
        struct Guard;
        impl Guard {
            fn on() -> Self {
                set_tracing(true);
                Guard
            }
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                set_tracing(false);
            }
        }
        let _guard = Guard::on();
        let module = parse(source, "trace-test").expect("must parse");
        let engine = crate::eval::Engine::new();
        let _ = engine.evaluate(&module);
        let trail = take_trail();
        let codes = trail.last().map(|t| t.codes.clone()).unwrap_or_default();
        (trail, codes)
    }

    #[test]
    fn trace_event_shape_is_closed() {
        // spec/observability.md: a TraceEvent carries exactly four
        // fields (phase, nodes, diags, codes) — values, sources,
        // paths, spans, and secrets can never enter telemetry because
        // the struct has nowhere to put them. Adding a field requires
        // an RFC, a spec update, and an update to this test.
        let event = TraceEvent {
            phase: "eval_done",
            nodes: 1,
            diags: 0,
            codes: vec![],
        };
        let json = serde_json::to_value(&event).expect("serializes");
        let obj = json.as_object().expect("object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["codes", "diags", "nodes", "phase"]);
    }

    #[test]
    fn trace_is_off_by_default() {
        let module = parse("port = 1\n", "trace-test").expect("must parse");
        let engine = crate::eval::Engine::new();
        let _ = engine.evaluate(&module);
        assert!(take_trail().is_empty());
        assert!(!ENABLED.with(Cell::get));
    }

    #[test]
    fn trace_carries_codes_never_values() {
        // Refused evaluation: only eval_start is recorded, with no codes.
        let (trail, codes) = codes_of("timeout = 5s + 512MiB\n");
        assert_eq!(trail.len(), 1, "{trail:?}");
        assert_eq!(trail[0].phase, "eval_start");
        assert!(codes.is_empty(), "{codes:?}");
        // Clean evaluation: start plus done with an empty code list.
        let (trail, codes) = codes_of("port = 8080\n");
        assert_eq!(trail.len(), 2, "{trail:?}");
        assert_eq!(trail[1].phase, "eval_done");
        assert!(codes.is_empty(), "{codes:?}");
        // The secret-carrying document evaluates (null indirection with
        // W001); the raw value appears nowhere in the trail.
        let (trail, _) = codes_of("token = secret(\"hunter2\")\n[out]\nv = token\n");
        let text = format!("{trail:?}");
        assert!(!text.contains("hunter2"), "{text}");
    }
}

//! Per-value layer provenance (LANG-F-014).
//!
//! The evaluated `Value` is layer-flattened by design: profile overlays and
//! patches are merged into one tree (highest priority wins; a profile type
//! change is not a warning under L-P09), and the JSON wire form carries no
//! record of which layer wrote a value. That is correct for the wire - a
//! receiver must not have to
//! re-implement merges - but an operator deciding how much to trust a value,
//! or a receiver auditing a peer document, needs to know whether a value came
//! from the base document, a profile, or a patch.
//!
//! This module records that provenance on demand (it costs nothing when off):
//! a thread-local map from value path to the layer that wrote it last, filled
//! while merges run and consumed by the inspection surface
//! (`inspect_path_with_layers`) or the audit trail. Merge semantics are
//! unchanged; only bookkeeping is added.

use std::cell::RefCell;
use std::collections::BTreeMap;

thread_local! {
    /// `None` when the recorder is off (the normal case). A running
    /// evaluation that requested layers owns the map until it finishes.
    static LAYERS: RefCell<Option<BTreeMap<String, String>>> = const { RefCell::new(None) };
}

/// Start recording for this thread. Returns `false` when a recorder is
/// already running (nested evaluations keep the outer one).
pub(crate) fn begin() -> bool {
    LAYERS.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_some() {
            return false;
        }
        *slot = Some(BTreeMap::new());
        true
    })
}

/// Stop recording and hand back the map, or an empty map when nothing ran.
pub(crate) fn finish() -> BTreeMap<String, String> {
    LAYERS.with(|cell| cell.borrow_mut().take().unwrap_or_default())
}

/// True while a recorder is active; merge sites check this before building
/// any path string, so the off case costs one thread-local read.
pub(crate) fn is_active() -> bool {
    LAYERS.with(|cell| cell.borrow().is_some())
}

/// Record that `layer` wrote the value at `path` (last writer wins).
pub(crate) fn record(path: &str, layer: &str) {
    if !is_active() {
        return;
    }
    LAYERS.with(|cell| {
        if let Some(map) = cell.borrow_mut().as_mut() {
            map.insert(path.to_string(), layer.to_string());
        }
    });
}

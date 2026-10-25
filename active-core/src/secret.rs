//! Structural secret-taint analysis (TOOLING DEBUG.001, INSPECT.001-005,
//! spec/debug.md §5, spec/lsp.md §2).
//!
//! The evaluator marks `secret(...)` results with `Value::Secret`, but
//! material also flows through plain nodes: a binding used as a locator,
//! an environment lookup feeding one, an interpolation embedding either.
//! Consumers that publish values (step traces, `inspect`, hover) must
//! recognize those flows structurally, because the runtime value carries
//! no marker. This module computes, from the module alone:
//!
//! * every node reachable from a `secret(...)` locator, plus every node
//!   derived from one (an ancestor container, an interpolation part, a
//!   reference);
//! * every literal string inside that set, so renderings can redact
//!   *substrings* (an interpolated prefix/suffix hides the literal);
//! * value paths for the document tree, including array elements, so a
//!   caller can ask whether one path is secret material.
//!
//! All redaction is deterministic: known secrets are applied
//! longest-first, then lexicographically.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use serde_json::Value as JsonValue;

use crate::ir::{InterpPart, Module, Node, NodeId, PatchOp, Value};

/// Replacement text for redacted material. Shared with the evaluator's
/// JSON mapping so every surface emits the same marker.
pub const MARKER: &str = "***SECRET***";

/// Direct value-producing children of a node. Schema nodes are
/// excluded: they describe contracts and never evaluate to document
/// data, so following them would taint unrelated values.
pub fn node_children(node: &Node) -> Vec<NodeId> {
    match node {
        Node::Literal(_) | Node::Ident(_) | Node::PatchDirective { .. } => Vec::new(),
        Node::Env { default, .. } => default.iter().copied().collect(),
        Node::Secret { locator } => vec![*locator],
        Node::ContentAddressed { media_type, data } => vec![*media_type, *data],
        Node::Ref(target) => vec![*target],
        Node::Interpolate { parts } => parts
            .iter()
            .filter_map(|part| match part {
                InterpPart::Expr(id) => Some(*id),
                InterpPart::Literal(_) => None,
            })
            .collect(),
        Node::Binary { lhs, rhs, .. } => vec![*lhs, *rhs],
        Node::Unary { expr, .. } => vec![*expr],
        Node::Array { elements } => elements.clone(),
        Node::Table { fields } => fields.values().copied().collect(),
        Node::Quantity { value, .. } => vec![*value],
        Node::ApplyConstraint { target, .. } => vec![*target],
        Node::SchemaField { .. } | Node::SchemaTable { .. } | Node::SchemaArray { .. } => {
            Vec::new()
        }
        Node::Semantic { inner, .. } => vec![*inner],
    }
}

/// Direct value children by module lookup (`NodeId`s dereference
/// through `Module::get`).
pub fn value_children(module: &Module, id: NodeId) -> Vec<NodeId> {
    module.get(id).map(node_children).unwrap_or_default()
}

/// Insert `root` and every value node reachable from it into `set`.
pub fn secret_subtree(module: &Module, root: NodeId, set: &mut HashSet<NodeId>) {
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !set.insert(id) {
            continue;
        }
        stack.extend(value_children(module, id));
    }
}

/// Patterns longer than this stay out of the automaton. A single very
/// long secret would otherwise cost one automaton state per byte
/// (tens of megabytes for a 500 KB value); `str::find` scans such a
/// pattern in linear time instead.
const MAX_AC_PATTERN: usize = 4096;

/// Per-pattern cursor over one marker-free segment: the next known
/// match start and whether the pattern has no further match in it.
/// Caching both keeps the linear fallback at O(text) per pattern
/// instead of rescanning the remainder after every match.
type PatternCursor = (Option<usize>, bool);

/// Next match of one linearly scanned pattern at or after `last`.
/// The cursor keeps each pattern's segment search linear: the search
/// resumes after the previous match and stops once the remainder is
/// too short.
fn linear_next(
    pattern: &str,
    segment: &str,
    last: usize,
    cursor: &mut PatternCursor,
) -> Option<(usize, usize)> {
    if cursor.1 {
        return None;
    }
    if pattern.len() > segment.len() - last {
        cursor.1 = true;
        return None;
    }
    let start = match cursor.0 {
        Some(start) if start >= last => start,
        _ => match segment[last..].find(pattern) {
            Some(relative) => {
                let start = last + relative;
                cursor.0 = Some(start);
                start
            }
            None => {
                cursor.1 = true;
                return None;
            }
        },
    };
    Some((start, start + pattern.len()))
}

/// Per-batch automaton cursor over one marker-free segment: the next
/// known match and whether the batch has no further match in it.
/// Caching the match keeps a distant match from re-scanning the suffix
/// for every earlier match.
type LevelCursor = (Option<(usize, usize)>, bool);

/// Next match of one batch automaton at or after `last`. The cursor
/// keeps the batch's segment search linear: a distant match is
/// remembered, and an exhausted batch is never scanned again.
fn level_next(
    automaton: &aho_corasick::AhoCorasick,
    segment: &str,
    last: usize,
    cursor: &mut LevelCursor,
) -> Option<(usize, usize)> {
    if cursor.1 {
        return None;
    }
    if let Some((start, end)) = cursor.0 {
        if start >= last {
            return Some((start, end));
        }
    }
    match automaton.find(&segment[last..]) {
        Some(found) => {
            let match_span = (last + found.start(), last + found.end());
            cursor.0 = Some(match_span);
            Some(match_span)
        }
        None => {
            cursor.1 = true;
            None
        }
    }
}

/// Keep the leftmost match; at the same start keep the longer one.
fn consider(start: usize, end: usize, best: &mut Option<(usize, usize)>) {
    let replace = match *best {
        None => true,
        Some((best_start, best_end)) => {
            start < best_start || (start == best_start && end > best_end)
        }
    };
    if replace {
        *best = Some((start, end));
    }
}

/// One log-structured batch of automaton-eligible values. The matcher
/// is built lazily and rebuilt only when the batch changes, so an
/// insert merges logarithmically and a scan visits O(log n) automata
/// instead of rescanning a growing pending set.
#[derive(Clone, Default)]
struct Level {
    values: Vec<String>,
    automaton: Option<std::sync::Arc<aho_corasick::AhoCorasick>>,
}

impl Level {
    fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Build the matcher for this batch. Values are sorted longest
    /// first so the automaton's leftmost-first tie-break keeps the
    /// longer match at an equal start.
    fn ensure(&mut self) -> Result<(), ()> {
        if self.values.is_empty() {
            self.automaton = None;
            return Ok(());
        }
        if self.automaton.is_some() {
            return Ok(());
        }
        self.values
            .sort_unstable_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        match aho_corasick::AhoCorasick::builder()
            .match_kind(aho_corasick::MatchKind::LeftmostFirst)
            // A long pattern would make the default automaton enumerate
            // a dense transition table. The noncontiguous NFA builds
            // and scans in linear time with bounded memory instead.
            .kind(Some(aho_corasick::AhoCorasickKind::NoncontiguousNFA))
            .build(self.values.iter().map(String::as_str))
        {
            Ok(built) => {
                self.automaton = Some(std::sync::Arc::new(built));
                Ok(())
            }
            Err(_) => Err(()),
        }
    }
}

/// A deterministic set of secret strings. Insertion is amortized
/// O(log n) (a HashSet guards duplicates; each value joins one
/// log-structured batch). Redaction scans at most O(log n) automata
/// plus one forward cursor pass per long pattern, so no insert pattern
/// costs a rescan of the text and no benign value is wholesale-marked.
#[derive(Clone)]
pub struct SecretRedactor {
    /// Authoritative set, insertion order.
    values: std::cell::RefCell<Vec<String>>,
    seen: HashSet<String>,
    /// Shortest value, so a text that cannot contain any secret is
    /// rejected in O(1).
    min_len: std::cell::Cell<usize>,
    /// Automaton batches, each covering a disjoint subset of values.
    levels: std::cell::RefCell<Vec<Level>>,
    /// Patterns longer than [`MAX_AC_PATTERN`], scanned linearly.
    long: std::cell::RefCell<Vec<String>>,
    /// A matcher could not be built. Scanning stays correct but linear;
    /// never report a text clean because construction failed.
    linear_only: std::cell::Cell<bool>,
    /// Lazily built matcher over the values at or above a length, for
    /// substring probes that must ignore very short values. Keyed by
    /// the requested minimum so one build serves a whole pass.
    long_filtered: std::cell::RefCell<Option<(usize, Box<SecretRedactor>)>>,
}

impl std::fmt::Debug for SecretRedactor {
    /// Counts only: values are secret material and must not reach a
    /// diagnostic or a failed assertion's output.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretRedactor")
            .field("values", &self.values.borrow().len())
            .field("levels", &self.levels.borrow().len())
            .field("linear_only", &self.linear_only.get())
            .finish()
    }
}

impl PartialEq for SecretRedactor {
    /// Set equality: two redactors with the same values compare equal
    /// regardless of insertion order or batch state.
    fn eq(&self, other: &Self) -> bool {
        self.seen == other.seen
    }
}

impl Eq for SecretRedactor {}

impl Default for SecretRedactor {
    fn default() -> Self {
        Self {
            values: std::cell::RefCell::new(Vec::new()),
            seen: HashSet::new(),
            min_len: std::cell::Cell::new(usize::MAX),
            levels: std::cell::RefCell::new(Vec::new()),
            long: std::cell::RefCell::new(Vec::new()),
            linear_only: std::cell::Cell::new(false),
            long_filtered: std::cell::RefCell::new(None),
        }
    }
}

impl SecretRedactor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, text: impl Into<String>) {
        let text = text.into();
        // Empty strings would match every position; they are not
        // material.
        if text.is_empty() || !self.seen.insert(text.clone()) {
            return;
        }
        self.min_len.set(self.min_len.get().min(text.len()));
        if text.len() > MAX_AC_PATTERN {
            self.long.borrow_mut().push(text.clone());
        } else {
            // Binary-counter placement: merge equal batches upward so
            // at most one batch per power-of-two size exists.
            let mut carry = vec![text.clone()];
            let mut levels = self.levels.borrow_mut();
            let mut index = 0;
            loop {
                if index == levels.len() {
                    levels.push(Level::default());
                }
                if levels[index].is_empty() {
                    levels[index].values = carry;
                    levels[index].automaton = None;
                    break;
                }
                let existing = std::mem::take(&mut levels[index].values);
                levels[index].automaton = None;
                carry.extend(existing);
                index += 1;
            }
        }
        self.values.borrow_mut().push(text);
    }

    /// Build every batch matcher. Called before a pass and before each
    /// scan, so a batch inserted after the last pass is covered.
    fn ensure_automata(&self) {
        if self.linear_only.get() {
            return;
        }
        let mut levels = self.levels.borrow_mut();
        for level in levels.iter_mut() {
            if level.ensure().is_err() {
                self.linear_only.set(true);
                for level in levels.iter_mut() {
                    level.automaton = None;
                }
                return;
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.values.borrow().is_empty()
    }

    pub fn len(&self) -> usize {
        self.values.borrow().len()
    }

    /// True when `text` is exactly one of the known values (no
    /// substring matching). Used where a literal duplicate must be
    /// replaced without over-redacting longer or shorter strings.
    pub fn contains_exact(&self, text: &str) -> bool {
        self.seen.contains(text)
    }

    /// True when `text` contains a known value of at least `min_bytes`.
    /// Very short values stay exact-match only: spreading a one- or
    /// two-byte secret as a substring would redact unrelated fragments
    /// of every string. Used by the debugger's duplicate pass, where
    /// the caller has no path context to redact a value in place.
    ///
    /// Scans through the same automata as [`Self::redact_text`]: a
    /// filtered matcher is built once per minimum length, so a pass
    /// over many tokens costs O(text) each instead of O(values ×
    /// text).
    pub fn contains_substring(&self, text: &str, min_bytes: usize) -> bool {
        if self.is_empty() || text.len() < min_bytes {
            return false;
        }
        if self.min_len.get() >= min_bytes {
            return self.redact_text(text).is_some();
        }
        let mut cache = self.long_filtered.borrow_mut();
        if cache.as_ref().is_none_or(|(min, _)| *min != min_bytes) {
            let mut filtered = SecretRedactor::new();
            for value in self.values.borrow().iter() {
                if value.len() >= min_bytes {
                    filtered.insert(value.clone());
                }
            }
            *cache = Some((min_bytes, Box::new(filtered)));
        }
        match cache.as_ref() {
            Some((_, filtered)) => filtered.redact_text(text).is_some(),
            None => false,
        }
    }

    pub fn merge(&mut self, other: &SecretRedactor) {
        if std::ptr::eq(self, other) {
            return;
        }
        let values = other.values.borrow().clone();
        for value in values {
            self.insert(value);
        }
    }

    /// Replace every known secret substring. Returns the redacted text,
    /// or None when nothing matched (so callers can keep their fast
    /// path and report taint honestly).
    ///
    /// Existing markers are opaque: the text is split on [`MARKER`] and
    /// only the non-marker segments are scanned. A secret containing
    /// the marker itself cannot be expressed without ambiguity; the
    /// marker wins, deterministically.
    pub fn redact_text(&self, text: &str) -> Option<String> {
        if text == MARKER || self.min_len.get() == usize::MAX {
            return None;
        }
        // A text shorter than the shortest secret cannot match; no
        // automaton build is needed for it.
        if self.min_len.get() > text.len() {
            return None;
        }
        self.ensure_automata();
        self.scan(text)
    }

    /// Marks the start of an output pass. Batch matchers are built once
    /// here, so the pass itself does no build work and repeated exports
    /// are call-order independent.
    pub fn begin_pass(&self) {
        self.ensure_automata();
    }

    /// Scan one text across the batch automata and the linear
    /// partitions. The leftmost match wins; at the same start the
    /// longest wins. A batch or long pattern that finds nothing at or
    /// after the current position retires for the rest of the segment,
    /// so an absent matcher cannot rescan the suffix per match.
    fn scan(&self, text: &str) -> Option<String> {
        let levels = self.levels.borrow();
        let long = self.long.borrow();
        let linear = self.linear_only.get();
        let mut out: Option<String> = None;
        let mut offset = 0usize;
        // Existing markers are opaque: scan only non-marker segments.
        for (index, segment) in text.split(MARKER).enumerate() {
            if index > 0 {
                if let Some(buffer) = out.as_mut() {
                    buffer.push_str(MARKER);
                }
            }
            // The first match in a later segment must keep everything
            // before it, including the markers already in the text.
            let prefix = offset;
            offset += segment.len() + MARKER.len();
            let mut long_cursors: Vec<PatternCursor> = vec![(None, false); long.len()];
            let mut level_cursors: Vec<LevelCursor> = vec![(None, false); levels.len()];
            let mut pattern_cursors: Vec<Vec<PatternCursor>> = if linear {
                levels
                    .iter()
                    .map(|level| vec![(None, false); level.values.len()])
                    .collect()
            } else {
                Vec::new()
            };
            let mut last = 0usize;
            loop {
                let mut best: Option<(usize, usize)> = None;
                for (level_index, level) in levels.iter().enumerate() {
                    if linear {
                        let cursors = &mut pattern_cursors[level_index];
                        if cursors.iter().all(|cursor| cursor.1) {
                            continue;
                        }
                        for (pattern, cursor) in level.values.iter().zip(cursors.iter_mut()) {
                            if let Some((start, end)) = linear_next(pattern, segment, last, cursor)
                            {
                                consider(start, end, &mut best);
                            }
                        }
                    } else if let Some(automaton) = level.automaton.as_ref() {
                        if let Some((start, end)) =
                            level_next(automaton, segment, last, &mut level_cursors[level_index])
                        {
                            consider(start, end, &mut best);
                        }
                    }
                }
                for (pattern, cursor) in long.iter().zip(long_cursors.iter_mut()) {
                    if let Some((start, end)) = linear_next(pattern, segment, last, cursor) {
                        consider(start, end, &mut best);
                    }
                }
                let Some((start, end)) = best else { break };
                let buffer = out.get_or_insert_with(|| {
                    let mut buffer = String::with_capacity(text.len());
                    buffer.push_str(&text[..prefix]);
                    buffer
                });
                buffer.push_str(&segment[last..start]);
                buffer.push_str(MARKER);
                last = end;
            }
            if let Some(buffer) = out.as_mut() {
                buffer.push_str(&segment[last..]);
            }
        }
        out
    }
    /// Replace secret substrings inside every string of a rendered JSON
    /// tree. Returns true when anything was replaced.
    pub fn redact(&self, value: &mut JsonValue) -> bool {
        self.redact_inner(value)
    }

    fn redact_inner(&self, value: &mut JsonValue) -> bool {
        match value {
            JsonValue::String(text) => match self.redact_text(text) {
                Some(redacted) => {
                    *text = redacted;
                    true
                }
                None => false,
            },
            JsonValue::Array(items) => {
                let mut any = false;
                for item in items {
                    any |= self.redact_inner(item);
                }
                any
            }
            JsonValue::Object(map) => {
                let mut any = false;
                for item in map.values_mut() {
                    any |= self.redact_inner(item);
                }
                any
            }
            _ => false,
        }
    }
}

/// The media type a consumer may see. A tainted identity redacts its
/// media type (the wire shape is shared by JSON, YAML, TOML, `Debug`,
/// and inspect).
pub fn exposed_media_type(tainted: bool, media_type: &str) -> &str {
    if tainted {
        MARKER
    } else {
        media_type
    }
}

/// Remove one path and everything below it.
fn remove_subtree(paths: &mut HashMap<String, NodeId>, path: &str) {
    if path.is_empty() {
        paths.clear();
        return;
    }
    let dotted = format!("{path}.");
    let indexed = format!("{path}[");
    paths.retain(|key, _| key != path && !key.starts_with(&dotted) && !key.starts_with(&indexed));
}

/// Split a path into its parent and last segment:
/// `outer.a` → (`outer`, `a`), `a.b[0]` → (`a.b`, `[0]`), `a` → (``, `a`).
fn split_parent(path: &str) -> (&str, &str) {
    let dot = path.rfind('.');
    let bracket = path.rfind('[');
    match (dot, bracket) {
        (Some(d), Some(b)) if d > b => (&path[..d], &path[d + 1..]),
        (_, Some(b)) => (&path[..b], &path[b..]),
        (Some(d), None) => (&path[..d], &path[d + 1..]),
        (None, None) => ("", path),
    }
}

/// Remove a path, shifting later array indices down when the path names
/// an element (the evaluator's `Remove` semantics).
fn remove_path(paths: &mut HashMap<String, NodeId>, path: &str) {
    let (array, last) = split_parent(path);
    let Some(rest) = last.strip_prefix('[') else {
        remove_subtree(paths, path);
        return;
    };
    let Some((digits, _)) = rest.split_once(']') else {
        remove_subtree(paths, path);
        return;
    };
    let Ok(index) = digits.parse::<usize>() else {
        remove_subtree(paths, path);
        return;
    };
    if array.is_empty() {
        remove_subtree(paths, path);
        return;
    }
    remove_subtree(paths, path);
    let prefix = format!("{array}[");
    let mut moved: Vec<(String, NodeId)> = Vec::new();
    paths.retain(|key, value| {
        let Some(rest) = key.strip_prefix(&prefix) else {
            return true;
        };
        let Some((digits, tail)) = rest.split_once(']') else {
            return true;
        };
        let Ok(current) = digits.parse::<usize>() else {
            return true;
        };
        if current > index {
            moved.push((format!("{array}[{}]{tail}", current - 1), *value));
            false
        } else {
            true
        }
    });
    paths.extend(moved);
}

/// Move one path and everything below it. The evaluator's `@rename`
/// treats `to` as a plain key in the **same parent** as `from`
/// (spec/patches.md), so the destination is `parent(from)` + `to`. A
/// missing source is a no-op (the evaluator warns W004 and leaves the
/// tree intact).
fn rename_subtree(paths: &mut HashMap<String, NodeId>, from: &str, to: &str) {
    // Renaming an array element is a no-op in the evaluator (W004):
    // never relocate taint to a fabricated key.
    if split_parent(from).1.starts_with('[') {
        return;
    }
    let dotted = format!("{from}.");
    let indexed = format!("{from}[");
    let source_exists = paths.contains_key(from)
        || paths
            .keys()
            .any(|key| key.starts_with(&dotted) || key.starts_with(&indexed));
    if !source_exists {
        return;
    }
    let (parent, _) = split_parent(from);
    let dest = if parent.is_empty() {
        to.to_string()
    } else {
        format!("{parent}.{to}")
    };
    remove_subtree(paths, &dest);
    let mut moved: Vec<(String, NodeId)> = Vec::new();
    paths.retain(|key, value| {
        if key == from {
            moved.push((dest.clone(), *value));
            false
        } else if let Some(suffix) = key.strip_prefix(&dotted) {
            moved.push((format!("{dest}.{suffix}"), *value));
            false
        } else if let Some(suffix) = key.strip_prefix(&indexed) {
            moved.push((format!("{dest}[{suffix}"), *value));
            false
        } else {
            true
        }
    });
    paths.extend(moved);
}

/// Whether `key` is `base` or sits below it.
fn path_under(key: &str, base: &str) -> bool {
    key == base
        || key
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
}

/// Remove a path from a path set, shifting later array indices when the
/// path names an element.
fn remove_from_set(set: &mut HashSet<String>, path: &str) {
    let (array, last) = split_parent(path);
    if let Some(rest) = last.strip_prefix('[') {
        if let Some((digits, _)) = rest.split_once(']') {
            if let Ok(index) = digits.parse::<usize>() {
                if !array.is_empty() {
                    let prefix = format!("{array}[");
                    let mut moved = Vec::new();
                    set.retain(|key| {
                        if path_under(key, path) {
                            return false;
                        }
                        if let Some(rest) = key.strip_prefix(&prefix) {
                            if let Some((digits, tail)) = rest.split_once(']') {
                                if let Ok(current) = digits.parse::<usize>() {
                                    if current > index {
                                        moved.push(format!("{array}[{}]{tail}", current - 1));
                                        return false;
                                    }
                                }
                            }
                        }
                        true
                    });
                    set.extend(moved);
                    return;
                }
            }
        }
    }
    set.retain(|key| !path_under(key, path));
}

/// Move a path set's entries with the evaluator's same-parent rename
/// rule.
fn rename_set(set: &mut HashSet<String>, from: &str, to: &str) {
    if split_parent(from).1.starts_with('[') {
        return;
    }
    if !set.iter().any(|key| path_under(key, from)) {
        return;
    }
    let (parent, _) = split_parent(from);
    let dest = if parent.is_empty() {
        to.to_string()
    } else {
        format!("{parent}.{to}")
    };
    set.retain(|key| !path_under(key, &dest));
    let moved: Vec<String> = set
        .iter()
        .filter(|key| path_under(key, from))
        .cloned()
        .collect();
    for key in moved {
        set.remove(&key);
        let suffix = &key[from.len()..];
        set.insert(format!("{dest}{suffix}"));
    }
}
const MAX_PATH_DEPTH: usize = 256;
const MAX_PATHS: usize = 100_000;
/// Bound transform maintenance: orphan patches on a huge document must
/// not turn analysis quadratic.
const MAX_TRANSFORM_WORK: usize = 2_000_000;

/// Walk a value subtree from `root` under `base`, registering every
/// table field, array element, and reference target. Returns true when
/// a bound stopped the walk (the map is incomplete).
///
/// * `overwrite` values replace a mapping at the same path (transforms
///   win); otherwise the first mapping wins (a table field beats a
///   `let` binding, exactly like the evaluator).
/// * `replace_subtree` clears the path's existing descendants first: a
///   patch `Set` replaces the value wholesale.
fn walk_paths(
    module: &Module,
    root: NodeId,
    base: &str,
    paths: &mut HashMap<String, NodeId>,
    overwrite: bool,
    replace_subtree: bool,
) -> bool {
    if replace_subtree {
        remove_subtree(paths, base);
    }
    let base_depth = if base.is_empty() {
        0
    } else {
        base.matches(['.', '[']).count() + 1
    };
    // `insert` is false while descending an `env.X ?: fallback`
    // expression: the path keeps the Env node (which the taint seeds
    // may mark), but the fallback's children are real value paths and
    // must be registered, or a material child inside a fallback table
    // or array has no path and exports raw.
    let mut stack = vec![(root, base.to_string(), base_depth, true)];
    let mut seen: HashSet<(NodeId, String, bool)> = HashSet::new();
    let mut truncated = false;
    while let Some((id, path, depth, insert)) = stack.pop() {
        if depth > MAX_PATH_DEPTH || paths.len() >= MAX_PATHS {
            truncated = true;
            continue;
        }
        if !seen.insert((id, path.clone(), insert)) {
            continue;
        }
        if insert {
            if overwrite {
                paths.insert(path.clone(), id);
            } else {
                paths.entry(path.clone()).or_insert(id);
            }
        }
        match module.get(id) {
            Some(Node::Table { fields }) => {
                for (key, child) in fields {
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    stack.push((*child, child_path, depth + 1, true));
                }
            }
            Some(Node::Array { elements }) => {
                for (index, child) in elements.iter().enumerate() {
                    stack.push((*child, format!("{path}[{index}]"), depth + 1, true));
                }
            }
            Some(Node::Ref(target)) => stack.push((*target, path, depth, insert)),
            // Defensive: the parser folds constraints into
            // `module.annotations` today, but the IR variant is public
            // and preserves the value shape.
            Some(Node::ApplyConstraint { target, .. }) => {
                stack.push((*target, path, depth, insert));
            }
            Some(Node::Semantic { inner, .. }) => {
                stack.push((*inner, path, depth, insert));
            }
            Some(Node::Env {
                default: Some(default),
                ..
            }) => stack.push((*default, path, depth, false)),
            _ => {}
        }
    }
    truncated
}

/// Normalize concrete array indices to `[]` (`arr[0][12].a` →
/// `arr[][].a`). Schema-default paths use the wildcard because one fill
/// expression can materialize at any element index.
fn wildcard_indices(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = String::with_capacity(path.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && j < bytes.len() && bytes[j] == b']' {
                out.push_str("[]");
                i = j + 1;
                continue;
            }
        }
        let ch = path[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Whether a patch Set destination is reachable in the current tree:
/// an existing scalar blocks a key/index descent, an index needs an
/// array container and an in-bounds slot, and a new final key needs a
/// table parent (patch application creates intermediate tables).
fn patch_destination_applies(module: &Module, paths: &HashMap<String, NodeId>, path: &str) -> bool {
    let is_scalar = |known: &str| {
        paths.get(known).is_some_and(|id| {
            // An env-fallback value can evaluate to a table; follow it
            // (`is_table_like`) instead of trusting the node kind.
            !matches!(module.get(*id), Some(Node::Array { .. })) && !is_table_like(module, *id)
        })
    };
    let Ok(segments) = crate::parser::validate_patch_path(path) else {
        return false;
    };
    let mut current = String::new();
    for (index, segment) in segments.iter().enumerate() {
        let last = index + 1 == segments.len();
        match segment {
            crate::parser::PathSeg::Key(key) => {
                if !current.is_empty() && is_scalar(&current) {
                    return false;
                }
                if last {
                    return true;
                }
                current = if current.is_empty() {
                    key.clone()
                } else {
                    format!("{current}.{key}")
                };
            }
            crate::parser::PathSeg::Index(wanted) => {
                let stem = format!("{current}[");
                let mut max_index: Option<usize> = None;
                for known in paths.keys() {
                    let Some(rest) = known.strip_prefix(&stem) else {
                        continue;
                    };
                    let Some(end) = rest.find(']') else {
                        continue;
                    };
                    if let Ok(value) = rest[..end].parse::<usize>() {
                        max_index = Some(max_index.map_or(value, |m| m.max(value)));
                    }
                }
                let Some(max_index) = max_index else {
                    return false;
                };
                if *wanted > max_index {
                    return false;
                }
                if last {
                    return true;
                }
                current = format!("{current}[{wanted}]");
            }
        }
    }
    false
}

/// Expand a default-path pattern (`arr[].a[0]`) over the known element
/// containers (`arr[0]`, `arr[1]`, …). No wildcard alias is emitted:
/// one alias for several concrete sub-paths would make materiality
/// depend on map iteration order.
fn expand_default_path(
    pattern: &str,
    data_paths: &HashSet<String>,
    work: &mut usize,
) -> Vec<String> {
    *work = work.saturating_add(data_paths.len().max(1));
    if *work > MAX_TRANSFORM_WORK {
        return vec![pattern.to_string()];
    }
    let Some(bracket) = pattern.find("[]") else {
        return vec![pattern.to_string()];
    };
    let prefix = &pattern[..bracket];
    let suffix = &pattern[bracket + 2..];
    let stem = format!("{prefix}[");
    let mut out = Vec::new();
    for data in data_paths {
        let Some(rest) = data.strip_prefix(&stem) else {
            continue;
        };
        let Some(end) = rest.find(']') else {
            continue;
        };
        let index = &rest[..end];
        if index.is_empty()
            || !index.bytes().all(|b| b.is_ascii_digit())
            || !rest[end + 1..].is_empty()
        {
            continue;
        }
        let concrete = format!("{prefix}[{index}]{suffix}");
        out.extend(expand_default_path(&concrete, data_paths, work));
    }
    if out.is_empty() {
        vec![pattern.to_string()]
    } else {
        out
    }
}

/// Materiality contributed by one schema default: each concrete
/// expansion is material when no explicit data covers that exact path
/// (the fill cannot run).
fn add_default_material_paths(
    path: &str,
    data_paths: &HashSet<String>,
    out: &mut HashSet<String>,
    work: &mut usize,
) {
    for concrete in expand_default_path(path, data_paths, work) {
        if !data_paths.contains(&concrete) {
            out.insert(concrete);
        }
    }
}

/// Canonical spelling of a patch path: parse and render through the
/// same `PathSeg` machinery the evaluator uses, so `arr[00]` and
/// `arr[0]` address one map entry. Invalid paths yield `None` (the
/// evaluator rejects them; analysis skips the transform).
fn canonical_patch_path(path: &str) -> Option<String> {
    let segments = crate::parser::validate_patch_path(path).ok()?;
    Some(crate::validate::render_path(&segments))
}

/// Canonical decimal spelling for concrete array indices in a query
/// path (`arr[00]` → `arr[0]`), matching the map keys the walk and
/// patch code produce.
fn canonical_indices(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = String::with_capacity(path.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let mut j = i + 1;
            if j < bytes.len() && bytes[j] == b'+' {
                j += 1;
            }
            let start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > start && j < bytes.len() && bytes[j] == b']' {
                if let Ok(value) = path[start..j].parse::<usize>() {
                    out.push('[');
                    out.push_str(&value.to_string());
                    out.push(']');
                    i = j + 1;
                    continue;
                }
            }
        }
        let ch = path[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Whether `from` names a path the evaluated tree owns (or a table
/// containing one). Bindings are not tree paths.
fn tree_path_exists(export_paths: &HashSet<String>, from: &str) -> bool {
    export_paths.contains(from) || export_paths.iter().any(|path| path_under(path, from))
}

/// Whether a path is material in the base tree, accepting the array
/// wildcard spelling on either side (`arr[].a` vs `arr[0].a`).
fn path_is_material(material_paths: &HashSet<String>, path: &str) -> bool {
    material_paths.contains(path) || material_paths.contains(&wildcard_indices(path))
}

/// Deterministic overlay entry: when one node is material at several
/// paths, the lexicographically smallest wins (map iteration order must
/// not decide a trace label).
fn set_material_path(map: &mut HashMap<NodeId, String>, node: NodeId, path: &str) {
    match map.get(&node) {
        Some(existing) if existing.as_str() <= path => {}
        _ => {
            map.insert(node, path.to_string());
        }
    }
}

/// Rename destinations inside a computed-profile path record.
fn rename_computed_paths(entries: &mut [(String, NodeId)], from: &str, to: &str) {
    if split_parent(from).1.starts_with('[') {
        return;
    }
    let (parent, _) = split_parent(from);
    let dest = if parent.is_empty() {
        to.to_string()
    } else {
        format!("{parent}.{to}")
    };
    for (path, _) in entries.iter_mut() {
        if path_under(path, from) {
            let suffix = &path[from.len()..];
            *path = format!("{dest}{suffix}");
        }
    }
}

/// Drop removed computed-profile paths and shift later array indices.
fn remove_computed_paths(entries: &mut Vec<(String, NodeId)>, path: &str) {
    let (array, last) = split_parent(path);
    let Some(rest) = last.strip_prefix('[') else {
        entries.retain(|(known, _)| !path_under(known, path));
        return;
    };
    let Some((digits, _)) = rest.split_once(']') else {
        entries.retain(|(known, _)| !path_under(known, path));
        return;
    };
    let Ok(index) = digits.parse::<usize>() else {
        entries.retain(|(known, _)| !path_under(known, path));
        return;
    };
    let prefix = format!("{array}[");
    entries.retain_mut(|(known, _)| {
        if path_under(known, path) {
            return false;
        }
        if array.is_empty() {
            return true;
        }
        let Some(tail) = known.strip_prefix(&prefix) else {
            return true;
        };
        let Some((digits, rest)) = tail.split_once(']') else {
            return true;
        };
        let Ok(current) = digits.parse::<usize>() else {
            return true;
        };
        if current > index {
            *known = format!("{prefix}{}]{}", current - 1, rest);
        }
        true
    });
}

/// Keep the trace-path overlay in step with a rename.
fn rename_material_node_paths(map: &mut HashMap<NodeId, String>, from: &str, to: &str) {
    // Renaming an array element is a W004 no-op in the evaluator.
    if split_parent(from).1.starts_with('[') {
        return;
    }
    let (parent, _) = split_parent(from);
    let dest = if parent.is_empty() {
        to.to_string()
    } else {
        format!("{parent}.{to}")
    };
    for path in map.values_mut() {
        if path_under(path, from) {
            let suffix = &path[from.len()..];
            *path = format!("{dest}{suffix}");
        }
    }
}

/// Keep the trace-path overlay in step with a removal, including the
/// evaluator's array-index shift.
fn remove_material_node_paths(map: &mut HashMap<NodeId, String>, path: &str) {
    let (array, last) = split_parent(path);
    let Some(rest) = last.strip_prefix('[') else {
        map.retain(|_, known| !path_under(known, path));
        return;
    };
    let Some((digits, _)) = rest.split_once(']') else {
        map.retain(|_, known| !path_under(known, path));
        return;
    };
    let Ok(index) = digits.parse::<usize>() else {
        map.retain(|_, known| !path_under(known, path));
        return;
    };
    let prefix = format!("{array}[");
    map.retain(|_, known| {
        if path_under(known, path) {
            return false;
        }
        if array.is_empty() {
            return true;
        }
        let Some(tail) = known.strip_prefix(&prefix) else {
            return true;
        };
        let Some((digits, rest)) = tail.split_once(']') else {
            return true;
        };
        let Ok(current) = digits.parse::<usize>() else {
            return true;
        };
        if current > index {
            *known = format!("{prefix}{}]{}", current - 1, rest);
        }
        true
    });
}

/// Fail closed after an analysis bound: every node and env read is
/// treated as material, so node-based surfaces (traces, provenance)
/// cannot publish an unprocessed profile value.
fn mark_all_material(module: &Module, nodes: &mut HashSet<NodeId>, envs: &mut HashSet<String>) {
    for (index, node) in module.arena.iter().enumerate() {
        nodes.insert(NodeId(index as u32));
        if let Node::Env { name, .. } = node {
            envs.insert(name.clone());
        }
    }
}

/// Bound on node visits across one analysis' static resolution. A
/// profile may reference a large shared subtree from many paths; the
/// cache resolves each node once, and this stops a crafted graph from
/// costing unbounded CPU.
const MAX_STATIC_WORK: usize = 2_000_000;
/// Bound on resolved bytes held by one analysis' resolver.
const MAX_STATIC_BYTES: usize = 16 * 1024 * 1024;
/// Recursion bound for one chain. Cycles are detected exactly by the
/// `visiting` set; this only keeps a crafted deep chain off the stack.
const MAX_STATIC_DEPTH: usize = 256;

/// Static string resolution through literals, references, bindings,
/// semantic wrappers, and interpolations of the same. Used to seed a
/// profile-supplied value at a material path without running the
/// evaluator. `None` when any part needs runtime input (env,
/// arithmetic, calls), when a reference cycle is met, or when the
/// shared budget is exhausted; evaluator seeds still cover every
/// shipped export path in those cases.
#[derive(Default)]
struct StaticStrings {
    cache: HashMap<NodeId, Option<Rc<str>>>,
    visiting: HashSet<NodeId>,
    work: usize,
    bytes: usize,
    exhausted: bool,
}

impl StaticStrings {
    fn get(&mut self, module: &Module, id: NodeId) -> Option<Rc<str>> {
        self.get_at(module, id, 0)
    }

    fn get_at(&mut self, module: &Module, id: NodeId, depth: usize) -> Option<Rc<str>> {
        if self.exhausted || depth > MAX_STATIC_DEPTH {
            return None;
        }
        if let Some(hit) = self.cache.get(&id) {
            return hit.clone();
        }
        // A reference cycle cannot produce text. Stop on the revisit
        // instead of counting hops, so every legal alias chain (a
        // `let` chain of any length) still resolves.
        if !self.visiting.insert(id) {
            return None;
        }
        self.work = self.work.saturating_add(1);
        if self.work > MAX_STATIC_WORK {
            self.exhausted = true;
            self.visiting.remove(&id);
            return None;
        }
        let resolved: Option<Rc<str>> = match module.get(id) {
            Some(Node::Literal(Value::String(text))) => {
                if !self.charge(text.len()) {
                    None
                } else {
                    Some(Rc::from(text.as_str()))
                }
            }
            Some(Node::Ref(target)) => self.get_at(module, *target, depth + 1),
            Some(Node::Ident(name)) => match module.bindings.get(name) {
                Some(target) => self.get_at(module, *target, depth + 1),
                None => None,
            },
            Some(Node::Semantic { inner, .. }) => self.get_at(module, *inner, depth + 1),
            Some(Node::Interpolate { parts }) => {
                let mut out = String::new();
                let mut complete = true;
                for part in parts {
                    match part {
                        crate::ir::InterpPart::Literal(text) => {
                            if !self.charge(text.len()) {
                                complete = false;
                                break;
                            }
                            out.push_str(text);
                        }
                        crate::ir::InterpPart::Expr(expr) => {
                            match self.get_at(module, *expr, depth + 1) {
                                // The child charged its own text; the
                                // concatenation adds the same bytes
                                // again to the retained output.
                                Some(text) => {
                                    if !self.charge(text.len()) {
                                        complete = false;
                                        break;
                                    }
                                    out.push_str(&text);
                                }
                                None => {
                                    complete = false;
                                    break;
                                }
                            }
                        }
                    }
                }
                if complete {
                    Some(Rc::from(out))
                } else {
                    None
                }
            }
            _ => None,
        };
        self.visiting.remove(&id);
        self.cache.insert(id, resolved.clone());
        resolved
    }

    /// Charge `bytes` against the shared budget before allocating.
    /// Exhaustion is sticky: every later resolution returns `None`, so
    /// the analysis fails closed deterministically instead of seeding
    /// whichever nodes happened to be visited first.
    fn charge(&mut self, bytes: usize) -> bool {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > MAX_STATIC_BYTES {
            self.exhausted = true;
            false
        } else {
            true
        }
    }
}

/// Names of every env read anywhere in a value subtree, including
/// operands, interpolation parts, quantities, blob media/data, and env
/// fallbacks. A material profile value makes every read it contains
/// material under the env-name rule. Results are cached per node: a
/// profile may reference one large shared subtree many times.
fn collect_env_names(
    module: &Module,
    root: NodeId,
    names: &mut HashSet<String>,
    cache: &mut HashMap<NodeId, HashSet<String>>,
) {
    if let Some(found) = cache.get(&root) {
        names.extend(found.iter().cloned());
        return;
    }
    let mut found: HashSet<String> = HashSet::new();
    let mut stack = vec![root];
    let mut seen: HashSet<NodeId> = HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Some(Node::Env { name, .. }) = module.get(id) {
            found.insert(name.clone());
        }
        stack.extend(value_children(module, id));
    }
    names.extend(found.iter().cloned());
    cache.insert(root, found);
}

/// Whether a value is a table after following references (profile
/// merges recurse into tables; they replace scalars). Follows the whole
/// chain with a visited set: an eight-hop cap would misclassify a deep
/// alias as a scalar, and the profile branch would then delete live
/// material paths the evaluator still merges. A reference cycle cannot
/// evaluate; return `true` (conservative merge) rather than replace.
fn is_table_like(module: &Module, id: NodeId) -> bool {
    let mut current = id;
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(current) {
            return true;
        }
        match module.get(current) {
            Some(Node::Table { .. }) => return true,
            Some(Node::Ref(target)) => current = *target,
            // The fallback of `env.X ?: expr` is the value when the
            // variable is unset; treat a table fallback as a table so a
            // profile merge matches what the evaluator can produce.
            Some(Node::Env {
                default: Some(default),
                ..
            }) => current = *default,
            Some(Node::ApplyConstraint { target, .. }) => current = *target,
            Some(Node::Semantic { inner, .. }) => current = *inner,
            _ => return false,
        }
    }
}

/// The profile selector the document carries: absent, statically
/// named, or computed (a value expression we cannot resolve).
enum ProfileSelection {
    None,
    Named(String),
    Computed,
}

/// Resolve a value expression to a static string when it has one:
/// a literal, an all-literal interpolation (an escaped `$` parses that
/// way), or a reference chain to either. Never resolves through an
/// expression part, which may be material.
fn static_string(module: &Module, id: NodeId) -> Option<String> {
    let mut current = id;
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(current) {
            return None;
        }
        match module.get(current) {
            Some(Node::Literal(Value::String(text))) => return Some(text.clone()),
            Some(Node::Ref(target)) => current = *target,
            Some(Node::Interpolate { parts }) => {
                let mut out = String::new();
                for part in parts {
                    match part {
                        InterpPart::Literal(text) => out.push_str(text),
                        InterpPart::Expr(_) => return None,
                    }
                }
                return Some(out);
            }
            _ => return None,
        }
    }
}

/// Follow the root `active_profile` node. A static string is named; any
/// other expression is computed.
fn profile_selection(module: &Module) -> ProfileSelection {
    let Some(root) = module.root else {
        return ProfileSelection::None;
    };
    let Some(Node::Table { fields }) = module.get(root) else {
        return ProfileSelection::None;
    };
    let Some(id) = fields.get("active_profile") else {
        return ProfileSelection::None;
    };
    match static_string(module, *id) {
        Some(name) => ProfileSelection::Named(name),
        None => ProfileSelection::Computed,
    }
}

/// Public helper for diagnostics: the selector's literal value when the
/// document spells one (never a computed value, which may be material).
pub fn active_profile_name(module: &Module) -> Option<String> {
    match profile_selection(module) {
        ProfileSelection::Named(name) => Some(name),
        _ => None,
    }
}

/// Map schema-default paths to their default expression nodes. Runs
/// after explicit values, so a document value always wins.
fn collect_default_paths(
    module: &Module,
    id: NodeId,
    prefix: &str,
    paths: &mut HashMap<String, NodeId>,
) -> bool {
    let mut truncated = false;
    match module.get(id) {
        Some(Node::SchemaTable { fields, .. }) => {
            for (key, child) in fields {
                let child_path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                truncated |= collect_default_paths(module, *child, &child_path, paths);
            }
        }
        Some(Node::SchemaField { field }) => {
            if let Some(default) = field.default {
                truncated |= walk_paths(module, default, prefix, paths, false, false);
            }
        }
        Some(Node::SchemaArray { element }) => {
            // Fills materialise inside an array element, so their paths
            // carry an index. Register a wildcard (`arr[].a`) and let
            // `path_is_secret` normalize concrete indices: the evaluated
            // tree has `arr[0].a`, which the map can never enumerate.
            let element_prefix = if prefix.is_empty() {
                "[]".to_string()
            } else {
                format!("{prefix}[]")
            };
            truncated |= collect_default_paths(module, *element, &element_prefix, paths);
        }
        _ => {}
    }
    truncated
}

/// Structural view of the secret material in one module.
#[derive(Debug, Clone, Default)]
pub struct SecretProfile {
    nodes: HashSet<NodeId>,
    paths: HashMap<String, NodeId>,
    /// Paths from **all** profiles that are material, used when the
    /// selector is computed: a set, never a last-wins map, so a later
    /// benign profile cannot hide an earlier material one.
    fallback_secret_paths: HashSet<String>,
    /// A bound stopped the walk, so unknown paths must fail closed.
    truncated: bool,
    literals: SecretRedactor,
    /// Environment names whose read sits at a material path (including
    /// profile-selected values). Provenance withholds these values even
    /// when the node itself is not globally tainted.
    material_envs: HashSet<String>,
    /// A material path for a node that is only path-tainted (a profile
    /// selection). Step traces use it so a shared benign node stays
    /// visible in the export while its material step redacts.
    material_node_paths: HashMap<NodeId, String>,
    /// Export-tree paths (root and selected profile), so a trace can
    /// tell a binding label from a data path.
    data_paths: HashSet<String>,
    /// Nodes still visible through a benign export path after every
    /// transform. A path-material value outside this set is material
    /// even when its source node is also used benignly elsewhere that
    /// the profile overrode.
    export_visible: HashSet<NodeId>,
    /// Nodes served at a material path in the final tree. Unlike
    /// [`Self::material_node_paths`] this is a set computed after every
    /// transform, so a patched-over path no longer counts.
    material_at_path: HashSet<NodeId>,
}

impl SecretProfile {
    /// Analyze a module with the document's own profile selector.
    pub fn analyze(module: &Module) -> Self {
        Self::analyze_with_profile(module, None)
    }

    /// Analyze a module, optionally with an engine/CLI profile override.
    /// Costs one pass over the arena plus one walk of the value tree.
    pub fn analyze_with_profile(module: &Module, override_profile: Option<&str>) -> Self {
        // Seeds: every node reachable from a `secret(...)` locator.
        let mut nodes: HashSet<NodeId> = HashSet::new();
        let mut material_envs: HashSet<String> = HashSet::new();
        let mut material_node_paths: HashMap<NodeId, String> = HashMap::new();
        for node in &module.arena {
            if let Node::Secret { locator } = node {
                secret_subtree(module, *locator, &mut nodes);
            }
        }

        // A variable that feeds a secret anywhere is secret everywhere
        // (spec/reproducibility.md §2): every `env` read of the same
        // name is material, even a later, unrelated-looking one.
        let mut env_names: HashSet<String> = HashSet::new();
        for id in &nodes {
            if let Some(Node::Env { name, .. }) = module.get(*id) {
                env_names.insert(name.clone());
            }
        }
        if !env_names.is_empty() {
            for (index, node) in module.arena.iter().enumerate() {
                if let Node::Env { name, .. } = node {
                    if env_names.contains(name) {
                        nodes.insert(NodeId(index as u32));
                    }
                }
            }
        }

        // Derived: an ancestor container, an interpolation, a reference
        // or an operator whose operand is material. Build the reverse
        // (child → parents) map once and walk upward from the seeds:
        // linear in the arena and exact, with no fixpoint rounds.
        if !nodes.is_empty() {
            let mut parents: Vec<Vec<NodeId>> = vec![Vec::new(); module.arena.len()];
            for (index, node) in module.arena.iter().enumerate() {
                let parent = NodeId(index as u32);
                for child in node_children(node) {
                    if let Some(slot) = parents.get_mut(child.0 as usize) {
                        slot.push(parent);
                    }
                }
            }
            let mut stack: Vec<NodeId> = nodes.iter().copied().collect();
            while let Some(id) = stack.pop() {
                if let Some(list) = parents.get(id.0 as usize) {
                    for parent in list {
                        if nodes.insert(*parent) {
                            stack.push(*parent);
                        }
                    }
                }
            }
        }

        // Authored strings inside material nodes: the locator literal,
        // a literal referenced by the locator, and so on. Interpolation
        // literal parts are deliberately absent — they are the benign
        // scaffolding, not the material.
        let mut literals = SecretRedactor::new();
        for id in &nodes {
            if let Some(Node::Literal(Value::String(text))) = module.get(*id) {
                literals.insert(text.clone());
            }
        }

        // Paths for every value node reachable from the root: tables,
        // array elements, and references (which keep the referrer's
        // path). The root walk runs FIRST: a table field wins ties
        // against a `let` binding, exactly like the evaluator. Binding
        // subtrees are walked too, so `t.a` inside a `let t = {…}` is
        // addressable and taint-checkable.
        let mut truncated = false;
        let mut paths: HashMap<String, NodeId> = HashMap::new();
        // Data paths are the exportable tree. Bindings are a separate
        // namespace: a `let` name colliding with a profile key must not
        // make that key look material.
        let mut data_paths: HashSet<String> = HashSet::new();
        if let Some(root) = module.root {
            truncated |= walk_paths(module, root, "", &mut paths, true, false);
            data_paths.extend(paths.keys().cloned());
        }
        // Paths the evaluated document actually provides: the root
        // tree plus profile overrides and patch destinations. Binding
        // names are deliberately absent: a `let` never shadows a
        // schema fill in the exported tree.
        let mut export_paths = data_paths.clone();
        for (name, id) in &module.bindings {
            truncated |= walk_paths(module, *id, name, &mut paths, false, false);
        }
        // A schema default may itself be material (an env locator) and
        // a profile may override that path before the fill runs.
        // Collect defaults into a scratch map before materiality is
        // computed; merge them into the path map after transforms so
        // patches still win.
        let mut default_paths: HashMap<String, NodeId> = HashMap::new();
        let defaults_truncated = match module.schema {
            Some(schema) => collect_default_paths(module, schema, "", &mut default_paths),
            None => false,
        };
        if defaults_truncated {
            truncated = true;
            default_paths.clear();
        }
        // Paths that are material in the base tree (explicit values and
        // schema defaults, before any profile selection). A profile
        // override replaces the value at a path; when that path is a
        // locator source (or derives from one), the replacement is
        // material too, whatever node the override names (`n = env.X`
        // under one profile, a literal under another). The selected
        // node therefore inherits the path's materiality; the root-only
        // node set cannot know about it.
        let mut material_paths: HashSet<String> = paths
            .iter()
            .filter(|(path, id)| data_paths.contains(*path) && nodes.contains(*id))
            .map(|(path, _)| path.clone())
            .collect();
        if !defaults_truncated {
            let mut default_work = 0usize;
            for (path, id) in &default_paths {
                if nodes.contains(id) {
                    add_default_material_paths(
                        path,
                        &data_paths,
                        &mut material_paths,
                        &mut default_work,
                    );
                }
            }
        }
        // Evaluation applies the selected profile first, then patches.
        // Profile tables merge (their fields override per key); a
        // profile scalar replaces. A computed selector cannot be
        // resolved, so **every** profile contributes material paths to
        // a taint-only set: the union never lets a later benign profile
        // hide an earlier material one. Patches run in source order:
        // `Set` replaces, `Remove` deletes (shifting array indices),
        // `Rename` moves within the same parent.
        let mut fallback_secret_paths: HashSet<String> = HashSet::new();
        let selection = match override_profile {
            Some(name) => ProfileSelection::Named(name.to_string()),
            None => profile_selection(module),
        };
        // A bound that already stopped the base walk makes every known
        // path unreliable. Profile paths must then stay unknown (fail
        // closed) instead of registering as benign values.
        let base_truncated = truncated;
        // Base values per path, before any profile field overwrites
        // them: merge-vs-replace is decided per path, not only at the
        // profile field's top level (a nested replacement under a
        // merged table still replaces its own value).
        let base_paths = paths.clone();
        // Every path contributed by a computed-selector profile: after
        // env-name propagation, a later-material node makes its path
        // material even though the profile was processed first (map
        // iteration order must not decide taint).
        let mut computed_profile_paths: Vec<(String, NodeId)> = Vec::new();
        // Shared work bound across profile fields: each field walks a
        // fresh map, so the per-map MAX_PATHS alone cannot bound the
        // merged analysis. On exhaustion, fail closed for the document.
        let mut profile_work = 0usize;
        let mut env_name_cache: HashMap<NodeId, HashSet<String>> = HashMap::new();
        // Profile merging is legitimately O(fields × paths) on large
        // documents; a separate, larger bound keeps normal 10k-key
        // profiles exact while still bounding a pathological merge.
        const MAX_PROFILE_WORK: usize = 20_000_000;
        match selection {
            ProfileSelection::Named(name) => {
                if let Some(id) = module.profiles.get(&name) {
                    if let Some(Node::Table { fields }) = module.get(*id) {
                        for (key, child) in fields {
                            let base_node = base_paths.get(key).copied();
                            // Merge only when BOTH sides are tables: a
                            // profile table over a scalar replaces the
                            // whole value, so its leaves are new.
                            let merge = is_table_like(module, *child)
                                && base_node.is_some_and(|b| is_table_like(module, b));
                            let mut profile_paths = HashMap::new();
                            let profile_truncated =
                                walk_paths(module, *child, key, &mut profile_paths, true, !merge);
                            truncated |= profile_truncated;
                            profile_work = profile_work
                                .saturating_add(profile_paths.len())
                                .saturating_add(paths.len());
                            if profile_work > MAX_PROFILE_WORK {
                                truncated = true;
                                paths.clear();
                                fallback_secret_paths.clear();
                                mark_all_material(module, &mut nodes, &mut material_envs);
                                break;
                            }
                            if base_truncated || profile_truncated {
                                // Fail closed: mark the whole selected
                                // subtree, keep every path material, and
                                // never publish them as known-benign.
                                for node in profile_paths.values() {
                                    secret_subtree(module, *node, &mut nodes);
                                }
                                fallback_secret_paths.extend(profile_paths.keys().cloned());
                                // Known base paths under the overridden
                                // key are stale: partial registration
                                // would bypass the global fail-closed
                                // check for them.
                                remove_subtree(&mut paths, key);
                                fallback_secret_paths.insert(key.clone());
                                continue;
                            }
                            // Per-path merge decision: a nested table at
                            // a scalar base path replaces that value.
                            let mut replaces: Vec<String> = Vec::new();
                            let mut replacement_prefixes: Vec<String> = Vec::new();
                            for (path, node) in &profile_paths {
                                let path_merge = is_table_like(module, *node)
                                    && base_paths
                                        .get(path)
                                        .is_some_and(|b| is_table_like(module, *b));
                                if !path_merge {
                                    replaces.push(path.clone());
                                    // A replacement of an existing
                                    // material value is material; a
                                    // profile-provided value at an
                                    // absent path answers for itself.
                                    let provided_only = !data_paths.contains(path);
                                    if !provided_only && path_is_material(&material_paths, path) {
                                        replacement_prefixes.push(path.clone());
                                    }
                                }
                            }
                            profile_work = profile_work
                                .saturating_add(replaces.len().saturating_mul(paths.len()));
                            if profile_work > MAX_PROFILE_WORK {
                                truncated = true;
                                paths.clear();
                                fallback_secret_paths.clear();
                                mark_all_material(module, &mut nodes, &mut material_envs);
                                break;
                            }
                            for path in &replaces {
                                remove_subtree(&mut paths, path);
                                // The replaced subtree is gone from the
                                // evaluated tree; its paths are re-added
                                // below only where the profile provides
                                // them.
                                export_paths.retain(|key| !path_under(key, path));
                            }
                            for (path, node) in &profile_paths {
                                let provided_only = !data_paths.contains(path);
                                let own = nodes.contains(node);
                                let inherited =
                                    !provided_only && path_is_material(&material_paths, path);
                                let under = replacement_prefixes
                                    .iter()
                                    .any(|prefix| path_under(path, prefix));
                                if own {
                                    nodes.insert(*node);
                                    fallback_secret_paths.insert(path.clone());
                                    set_material_path(&mut material_node_paths, *node, path);
                                    collect_env_names(
                                        module,
                                        *node,
                                        &mut material_envs,
                                        &mut env_name_cache,
                                    );
                                } else if inherited || under {
                                    // Path-inherited materiality stays
                                    // path-based: exports and debug
                                    // consult the path, so a shared
                                    // literal is not globally marked.
                                    fallback_secret_paths.insert(path.clone());
                                    set_material_path(&mut material_node_paths, *node, path);
                                    collect_env_names(
                                        module,
                                        *node,
                                        &mut material_envs,
                                        &mut env_name_cache,
                                    );
                                }
                            }
                            let profile_keys: Vec<String> = profile_paths.keys().cloned().collect();
                            paths.extend(profile_paths);
                            export_paths.extend(profile_keys);
                        }
                    }
                }
            }
            ProfileSelection::Computed => {
                'profiles: for id in module.profiles.values() {
                    if let Some(Node::Table { fields }) = module.get(*id) {
                        for (key, child) in fields {
                            let base_node = base_paths.get(key).copied();
                            let merge = is_table_like(module, *child)
                                && base_node.is_some_and(|b| is_table_like(module, b));
                            let mut profile_paths = HashMap::new();
                            let profile_truncated =
                                walk_paths(module, *child, key, &mut profile_paths, true, !merge);
                            truncated |= profile_truncated;
                            profile_work = profile_work
                                .saturating_add(profile_paths.len())
                                .saturating_add(paths.len());
                            if profile_work > MAX_PROFILE_WORK {
                                truncated = true;
                                paths.clear();
                                fallback_secret_paths.clear();
                                mark_all_material(module, &mut nodes, &mut material_envs);
                                break 'profiles;
                            }
                            if base_truncated || profile_truncated {
                                for node in profile_paths.values() {
                                    secret_subtree(module, *node, &mut nodes);
                                }
                                fallback_secret_paths.extend(profile_paths.keys().cloned());
                                remove_subtree(&mut paths, key);
                                fallback_secret_paths.insert(key.clone());
                                continue;
                            }
                            // A computed selector is a fail-closed
                            // union for materiality, but it does not
                            // prove any one profile ran: profile paths
                            // are not export/rename candidates.
                            let mut replacement_prefixes: Vec<String> = Vec::new();
                            computed_profile_paths
                                .extend(profile_paths.iter().map(|(p, n)| (p.clone(), *n)));
                            for (path, node) in &profile_paths {
                                let path_merge = is_table_like(module, *node)
                                    && base_paths
                                        .get(path)
                                        .is_some_and(|b| is_table_like(module, *b));
                                if path_merge {
                                    continue;
                                }
                                let provided_only = !data_paths.contains(path);
                                if !provided_only && path_is_material(&material_paths, path) {
                                    replacement_prefixes.push(path.clone());
                                }
                            }
                            for (path, node) in profile_paths {
                                let provided_only = !data_paths.contains(&path);
                                let own = nodes.contains(&node);
                                let inherited =
                                    !provided_only && path_is_material(&material_paths, &path);
                                let under = replacement_prefixes
                                    .iter()
                                    .any(|prefix| path_under(&path, prefix));
                                if own || inherited || under {
                                    set_material_path(&mut material_node_paths, node, &path);
                                    fallback_secret_paths.insert(path.clone());
                                    collect_env_names(
                                        module,
                                        node,
                                        &mut material_envs,
                                        &mut env_name_cache,
                                    );
                                    if own {
                                        nodes.insert(node);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            ProfileSelection::None => {}
        }
        // A variable that is material at one read is material at every
        // read (spec/reproducibility.md §2): propagate profile-selected
        // env names to all of their Env nodes before the final scan.
        let mut propagated_envs: HashSet<NodeId> = HashSet::new();
        for (index, node) in module.arena.iter().enumerate() {
            if let Node::Env { name, .. } = node {
                if material_envs.contains(name) {
                    let id = NodeId(index as u32);
                    if nodes.insert(id) {
                        propagated_envs.insert(id);
                    }
                }
            }
        }
        // An expression that embeds a newly material env read is
        // material too (interpolation, operator, quantity, blob). The
        // upward closure ran before the profile, so extend it now.
        if !propagated_envs.is_empty() {
            let mut parents: Vec<Vec<NodeId>> = vec![Vec::new(); module.arena.len()];
            for (index, node) in module.arena.iter().enumerate() {
                let parent = NodeId(index as u32);
                for child in node_children(node) {
                    if let Some(slot) = parents.get_mut(child.0 as usize) {
                        slot.push(parent);
                    }
                }
            }
            let mut stack: Vec<NodeId> = propagated_envs.into_iter().collect();
            while let Some(id) = stack.pop() {
                if let Some(list) = parents.get(id.0 as usize) {
                    for parent in list {
                        if nodes.insert(*parent) {
                            stack.push(*parent);
                        }
                    }
                }
            }
        }
        // A path from a computed-selector profile whose node became
        // material late (a second profile placed the same env name at
        // a material path) is material too.
        for (path, node) in &computed_profile_paths {
            if nodes.contains(node) {
                fallback_secret_paths.insert(path.clone());
            }
        }
        let mut work = 0usize;
        for id in &module.patches {
            work = work.saturating_add(
                paths
                    .len()
                    .saturating_add(fallback_secret_paths.len())
                    .max(1),
            );
            if work > MAX_TRANSFORM_WORK {
                // Fail closed: known paths may now be stale, so forget
                // them and let unknown paths report material. Defaults
                // must not re-register paths the abandoned transforms
                // could have made material.
                paths.clear();
                fallback_secret_paths.clear();
                mark_all_material(module, &mut nodes, &mut material_envs);
                truncated = true;
                break;
            }
            match module.get(*id) {
                Some(Node::PatchDirective {
                    op: PatchOp::Set { path, value },
                }) => {
                    let Some(path) = canonical_patch_path(path) else {
                        continue;
                    };
                    // Model success before registering: a failed Set
                    // (W004) does not create the path, while a
                    // successful one is a real rename/default
                    // candidate.
                    let applies = patch_destination_applies(module, &paths, &path);
                    truncated |= walk_paths(module, *value, &path, &mut paths, true, true);
                    // A patch replaces the evaluated value; old
                    // fallback paths at/under it are stale, and the
                    // patch expression's material paths take over.
                    fallback_secret_paths.retain(|known| !path_under(known, &path));
                    let mut patched = HashMap::new();
                    truncated |= walk_paths(module, *value, &path, &mut patched, true, true);
                    if applies {
                        // A Set replaces the subtree: the old
                        // descendants are no longer part of the tree.
                        export_paths.retain(|key| !path_under(key, &path));
                        export_paths.extend(patched.keys().cloned());
                        // A Set replaces the subtree: profile paths
                        // under it are stale, and so is the old value's
                        // step label (the new value answers for itself).
                        computed_profile_paths.retain(|(known, _)| !path_under(known, &path));
                        remove_material_node_paths(&mut material_node_paths, &path);
                    }
                    for (known, node) in patched {
                        if nodes.contains(&node) {
                            fallback_secret_paths.insert(known);
                        }
                    }
                }
                Some(Node::PatchDirective {
                    op: PatchOp::Remove { path },
                }) => {
                    let Some(path) = canonical_patch_path(path) else {
                        continue;
                    };
                    remove_path(&mut paths, &path);
                    remove_from_set(&mut fallback_secret_paths, &path);
                    remove_from_set(&mut export_paths, &path);
                    remove_material_node_paths(&mut material_node_paths, &path);
                    remove_computed_paths(&mut computed_profile_paths, &path);
                }
                Some(Node::PatchDirective {
                    op: PatchOp::Rename { from, to },
                }) => {
                    // Canonicalize the source (indices); the target is
                    // a key the evaluator accepts verbatim (`123` is a
                    // valid key, though not a segment shape accepted by
                    // `validate_patch_path`).
                    let from = canonical_patch_path(from).unwrap_or_else(|| from.clone());
                    let to = to.clone();
                    // Renames touch evaluated-tree keys only. A binding
                    // is not a tree key: the evaluator warns W004 and
                    // leaves the destination in place, so the taint map
                    // must not move the destination's materiality.
                    if !tree_path_exists(&export_paths, &from) {
                        // A computed selector may resolve to a profile
                        // that defines the source at runtime. Fail
                        // closed: carry the source's materiality to the
                        // destination instead of clearing it.
                        let root = from.split(['.', '[']).next().unwrap_or(from.as_str());
                        let profile_defined = module.profiles.values().any(|id| {
                            matches!(
                                module.get(*id),
                                Some(Node::Table { fields, .. }) if fields.contains_key(root)
                            )
                        });
                        if profile_defined {
                            // Same-parent destination, matching the
                            // evaluator (`a.x` renamed to `zz` becomes
                            // `a.zz`, never a bare top-level key).
                            let (parent, _) = split_parent(&from);
                            let dest = if parent.is_empty() {
                                to.clone()
                            } else {
                                format!("{parent}.{to}")
                            };
                            let mut affected: Vec<String> = fallback_secret_paths
                                .iter()
                                .filter(|path| path_under(path, &from))
                                .cloned()
                                .collect();
                            if paths.get(&from).is_some_and(|id| nodes.contains(id)) {
                                affected.push(from.clone());
                            }
                            let mut moved = false;
                            for path in affected {
                                moved = true;
                                let suffix = &path[from.len()..];
                                fallback_secret_paths.insert(format!("{dest}{suffix}"));
                            }
                            if moved {
                                fallback_secret_paths.insert(dest);
                            }
                            rename_material_node_paths(&mut material_node_paths, &from, &to);
                        }
                        continue;
                    }
                    rename_subtree(&mut paths, &from, &to);
                    rename_set(&mut fallback_secret_paths, &from, &to);
                    rename_set(&mut export_paths, &from, &to);
                    rename_material_node_paths(&mut material_node_paths, &from, &to);
                    rename_computed_paths(&mut computed_profile_paths, &from, &to);
                }
                _ => {}
            }
        }
        // Schema defaults fill absent paths during evaluation; explicit
        // values and transforms (inserted above) win. When any bound
        // stopped a walk, skip defaults: registering an unvisited path
        // as a benign default would defeat fail-closed analysis. Each
        // pattern expands over known element containers, so materiality
        // is per element and never depends on map iteration order.
        if !truncated {
            // Containers may be introduced by profiles or successful
            // patch Sets, not only by the root walk: expand defaults
            // over every container the evaluated tree can have.
            let mut expansion_paths: HashSet<String> = paths.keys().cloned().collect();
            expansion_paths.extend(computed_profile_paths.iter().map(|(path, _)| path.clone()));
            let mut expansion_work = 0usize;
            for (path, node) in default_paths {
                for concrete in expand_default_path(&path, &expansion_paths, &mut expansion_work) {
                    if export_paths.contains(&concrete) {
                        // Explicit values, profile overrides, and patch
                        // destinations win over a fill.
                        paths.entry(concrete.clone()).or_insert(node);
                    } else {
                        // A binding-only entry is not the exported
                        // value: the evaluator fills this path, so the
                        // fill node wins over the placeholder.
                        paths.insert(concrete.clone(), node);
                    }
                    // The filled path is part of the evaluated document,
                    // so a node there can be benignly export-visible.
                    export_paths.insert(concrete);
                }
            }
        }

        // Environment reads that are material anywhere (node-level
        // seeds or a path-tainted profile selection) are withheld by
        // provenance even when their node is shared with a benign read.
        for (index, node) in module.arena.iter().enumerate() {
            if let Node::Env { name, .. } = node {
                if nodes.contains(&NodeId(index as u32)) {
                    material_envs.insert(name.clone());
                }
            }
        }
        // A node is export-visible when some path the evaluated document
        // actually provides maps to it and neither the node nor that
        // path is material. `paths` also holds binding names and stale
        // nodes from failed Sets, so visibility is filtered by
        // `export_paths` (root tree + the selected profile's paths +
        // successful patch destinations + filled defaults).
        //
        // Visibility suppresses *literal* seeding only. The path-aware
        // renderer redacts a material path on its own; the seed exists
        // to catch copies of the text elsewhere (`dup`, a blob payload).
        // When the same node is still exported through a benign path,
        // its text legitimately appears there, so spreading the seed
        // would corrupt that benign value.
        let mut export_visible: HashSet<NodeId> = HashSet::new();
        for (path, id) in &paths {
            if !export_paths.contains(path) {
                continue;
            }
            if !nodes.contains(id) && !fallback_secret_paths.contains(path) {
                export_visible.insert(*id);
            }
        }
        // Every material path's current value is material: read the node
        // from the post-transform map, so a value a later profile or
        // patch moved or replaced is judged where it now is.
        let mut material_at_path: HashSet<NodeId> = HashSet::new();
        let mut static_strings = StaticStrings::default();
        for path in &fallback_secret_paths {
            let Some(node) = paths.get(path).copied() else {
                continue;
            };
            material_at_path.insert(node);
            if export_visible.contains(&node) {
                continue;
            }
            let Some(text) = static_strings.get(module, node) else {
                continue;
            };
            // A value that is material only through its path seeds when
            // it is long enough not to corrupt unrelated text; a
            // structurally secret value seeds at any length.
            if !nodes.contains(&node) && text.len() < 4 {
                continue;
            }
            literals.insert(text.to_string());
        }
        // Structural material: every literal inside a locator subtree,
        // including nodes that never enter the exported tree.
        for id in &nodes {
            if export_visible.contains(id) {
                continue;
            }
            if let Some(text) = static_strings.get(module, *id) {
                literals.insert(text.to_string());
            }
        }
        // A computed selector cannot prove which profile ran, so its
        // paths stay out of `paths` and `export_paths`; the recorded
        // node is nevertheless material under every possible selection.
        // Seed it directly (the union is deliberately conservative).
        for node in material_node_paths.keys() {
            material_at_path.insert(*node);
            if export_visible.contains(node) {
                continue;
            }
            let Some(text) = static_strings.get(module, *node) else {
                continue;
            };
            if !nodes.contains(node) && text.len() < 4 {
                continue;
            }
            literals.insert(text.to_string());
        }
        // An exhausted static resolver cannot know which values it
        // missed. Fail closed for the whole analysis: every literal is
        // material and every path is unknown, so no surface can publish
        // a value based on which nodes happened to be visited first.
        if static_strings.exhausted {
            truncated = true;
            mark_all_material(module, &mut nodes, &mut material_envs);
            export_visible.clear();
            material_at_path.extend(nodes.iter().copied());
            for node in &module.arena {
                if let Node::Literal(Value::String(text)) = node {
                    literals.insert(text.clone());
                }
            }
        }
        Self {
            nodes,
            paths,
            fallback_secret_paths,
            truncated,
            literals,
            material_envs,
            material_node_paths,
            data_paths,
            export_visible,
            material_at_path,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Analyze with the environment snapshot and runtime seeds that
    /// produced a tree. Every export surface needs this trio; keeping it
    /// in one place stops a surface from absorbing only part of it.
    pub fn with_context(
        module: &Module,
        override_profile: Option<&str>,
        env: Option<&HashMap<String, String>>,
        seeds: Option<&SecretRedactor>,
    ) -> Self {
        let mut profile = Self::analyze_with_profile(module, override_profile);
        if let Some(env) = env {
            profile.absorb_environment(env);
        }
        if let Some(seeds) = seeds {
            profile.absorb_secrets(seeds);
        }
        profile
    }

    /// Analyze for an engine and absorb its environment plus the seeds
    /// from its evaluation.
    pub fn for_engine(
        module: &Module,
        engine: &crate::eval::Engine,
        seeds: &SecretRedactor,
    ) -> Self {
        Self::with_context(
            module,
            engine.profile_override.as_deref(),
            Some(&engine.env),
            Some(seeds),
        )
    }

    /// True when an environment read of this name is material at any
    /// path, including a profile-selected value that is only
    /// path-tainted.
    pub fn env_is_material(&self, name: &str) -> bool {
        self.material_envs.contains(name)
    }

    /// Insert runtime secret seeds collected by the evaluator. Seeds
    /// cover material node values that never enter the exported tree
    /// (a `let` binding, a computed interpolation, a lazy host env
    /// lookup), so any copy of their text redacts on every surface.
    pub fn absorb_secrets(&mut self, seeds: &SecretRedactor) {
        self.literals.merge(seeds);
    }

    /// Insert every material environment value into the literal set.
    /// A runtime secret can be copied into an exported value through a
    /// path the export tree does not carry (a `let` binding, a profile
    /// replacement); seeding the value here makes the copy redact
    /// wherever it surfaces. Call after analysis, before export.
    pub fn absorb_environment(&mut self, env: &HashMap<String, String>) {
        let values: Vec<String> = self
            .material_envs
            .iter()
            .filter_map(|name| env.get(name).cloned())
            .collect();
        for value in values {
            self.literals.insert(value);
        }
    }

    /// True when the node is still rendered through a benign export path
    /// after every transform (the value is legitimately visible there).
    pub fn node_is_export_visible(&self, id: NodeId) -> bool {
        self.export_visible.contains(&id)
    }

    /// The path at which a node is material without being globally
    /// marked (a profile selection), for step-trace rendering.
    pub fn material_path_for(&self, id: NodeId) -> Option<&str> {
        self.material_node_paths.get(&id).map(String::as_str)
    }

    /// True when the node is served at a material path in the final
    /// tree, even if the node is also used benignly elsewhere. Computed
    /// after every profile and patch, so a replaced value drops out.
    pub fn node_is_path_material(&self, id: NodeId) -> bool {
        self.material_at_path.contains(&id)
    }

    /// True for a path the exported tree owns (root or selected
    /// profile), false for a binding-only label.
    pub fn is_data_path(&self, path: &str) -> bool {
        self.data_paths.contains(path)
    }

    /// True when the node's value is secret material or derives from it.
    pub fn node_is_secret(&self, id: NodeId) -> bool {
        self.nodes.contains(&id)
    }

    /// Node addressed by a dotted/indexed path (`a.b[0]`), when known.
    pub fn path_node(&self, path: &str) -> Option<NodeId> {
        self.paths.get(path).copied()
    }

    /// True when the path's value is secret material or derives from it.
    pub fn path_is_secret(&self, path: &str) -> bool {
        // Patch paths and caller queries may spell indices loosely
        // (`arr[00]`, `arr[+0]`); the map uses canonical decimals.
        let canonical = canonical_indices(path);
        let path = canonical.as_str();
        // The union set wins over a benign base mapping (computed
        // selectors must fail closed).
        if self.fallback_secret_paths.contains(path) {
            return true;
        }
        let wildcard = wildcard_indices(path);
        if let Some(id) = self.paths.get(path) {
            // A known concrete path decides for itself: an explicit
            // value wins over a wildcard schema default that cannot
            // fill it (`arr[0].a` written vs `arr[].a` default).
            return self.nodes.contains(id);
        }
        if self.fallback_secret_paths.contains(&wildcard) {
            return true;
        }
        // Unknown exact path: a wildcard default (`arr[].a`) applies.
        if wildcard != path && self.path_at_is_secret(&wildcard) {
            return true;
        }
        // A path the walk registered and did not mark is benign; an
        // unknown path after a bound stopped the walk fails closed.
        !self.paths.contains_key(&wildcard) && self.truncated
    }

    fn path_at_is_secret(&self, path: &str) -> bool {
        self.paths
            .get(path)
            .is_some_and(|id| self.nodes.contains(id))
            || self.fallback_secret_paths.contains(path)
    }

    /// Authored literal strings inside material nodes.
    pub fn literals(&self) -> &SecretRedactor {
        &self.literals
    }

    /// Marks the start of an output pass. Pending literals are folded
    /// into the automaton once here; the matcher keeps no other pass
    /// state, so repeated exports are call-order independent.
    pub fn begin_redaction_pass(&self) {
        self.literals.begin_pass();
    }

    /// Redact secret substrings in a rendered tree.
    pub fn redact_json(&self, value: &mut JsonValue) -> bool {
        self.literals.redact(value)
    }

    /// Redact secret substrings in text.
    pub fn redact_text(&self, text: &str) -> Option<String> {
        self.literals.redact_text(text)
    }
}

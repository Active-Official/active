//! Unification: the one table merge operation (L-P09).
//!
//! Every source merge in the language is this operation with a priority
//! attached to each side. Tables merge recursively; equal values agree;
//! different values at equal priority are a conflict; a higher priority
//! wins and records the losing source. Provenance is a bounded side map
//! (path -> winner + losers), never part of the exported value.
//!
//! Priority order: `Default` < `Include` < `Local` < `Profile`.
//!
//! Material is never compared: secrets are never equal (even to
//! themselves), and tainted blobs are never equal, so a merge can never
//! become an equality oracle. Unification checks shape and priority; it
//! does not read secret bytes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ir::Value;

/// Largest provenance map kept in memory. Beyond this the map reports
/// truncation; a merge never fails just because provenance is large.
pub const MAX_PROVENANCE_PATHS: usize = 65_536;
/// Bytes of path and source text one provenance map may retain. Paths are
/// caller-controlled, so the count cap alone does not bound memory: 65k
/// paths built from long keys would otherwise allocate gigabytes.
pub const MAX_PROVENANCE_BYTES: usize = 8 * 1024 * 1024;
/// Longest file or label text retained on one source identity.
pub const MAX_SOURCE_TEXT: usize = 4_096;
/// Deepest table or array nesting a merge walks. Matches the evaluator's
/// depth budget (X003), so a composed tree that could not evaluate also
/// cannot overflow the merge stack.
pub const MAX_MERGE_DEPTH: usize = 256;
/// Values one merge may visit across both inputs.
pub const MAX_MERGE_NODES: u64 = 1_000_000;
/// Clone budget for one merge, mirroring the X006 total-size ceiling.
pub const MAX_MERGE_BYTES: u64 = 100_000_000;
/// Most losing sources retained per path.
pub const MAX_LOSERS: usize = 16;

/// Where a merged value came from, strongest last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergePriority {
    Default,
    Include,
    Local,
    Profile,
}

/// One side of a merge: priority, source location, and a label for
/// diagnostics (`include "lib.active"`, `[server]`, `profile "prod"`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeSource {
    /// Serialized as `kind`, matching the published provenance shape.
    #[serde(rename = "kind")]
    pub priority: MergePriority,
    pub file: Option<String>,
    pub line: u32,
    pub label: String,
}

impl MergeSource {
    pub fn new(
        priority: MergePriority,
        file: Option<String>,
        line: u32,
        label: impl Into<String>,
    ) -> Self {
        Self {
            priority,
            file: file.map(bounded_source_text),
            line,
            label: bounded_source_text(label.into()),
        }
    }

    /// Bound text that arrived through deserialization or a hand-built
    /// record, where [`Self::new`]'s cap was bypassed.
    fn bounded(mut self) -> Self {
        self.file = self.file.map(bounded_source_text);
        self.label = bounded_source_text(self.label);
        self
    }
}

/// Truncate source text at the retained cap, on a character boundary so
/// the result stays valid UTF-8.
fn bounded_source_text(mut text: String) -> String {
    if text.len() <= MAX_SOURCE_TEXT {
        return text;
    }
    let mut cut = MAX_SOURCE_TEXT;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    text
}

/// Which source won a path and which sources it overrode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeRecord {
    pub winner: MergeSource,
    pub losers: Vec<MergeSource>,
    /// True when the loser list hit [`MAX_LOSERS`].
    #[serde(default)]
    pub losers_truncated: bool,
}

/// Path-indexed merge provenance, bounded and deterministic (BTreeMap).
#[derive(Debug, Clone)]
pub struct MergeProvenance {
    records: BTreeMap<String, MergeRecord>,
    limit: usize,
    byte_limit: usize,
    bytes: usize,
    truncated: bool,
}

impl Default for MergeProvenance {
    fn default() -> Self {
        Self::new()
    }
}

impl MergeProvenance {
    pub fn new() -> Self {
        Self {
            records: BTreeMap::new(),
            limit: MAX_PROVENANCE_PATHS,
            byte_limit: MAX_PROVENANCE_BYTES,
            bytes: 0,
            truncated: false,
        }
    }

    /// A provenance map with an explicit path cap (tests and bounded hosts).
    pub fn with_limit(limit: usize) -> Self {
        Self {
            records: BTreeMap::new(),
            limit,
            byte_limit: MAX_PROVENANCE_BYTES,
            bytes: 0,
            truncated: false,
        }
    }

    /// A provenance map with explicit path and byte caps.
    pub fn with_limits(paths: usize, bytes: usize) -> Self {
        Self {
            records: BTreeMap::new(),
            limit: paths,
            byte_limit: bytes,
            bytes: 0,
            truncated: false,
        }
    }

    /// Look up one path. When [`Self::truncated`] is true, `None` means
    /// "not recorded or dropped at the cap"; consult the flag before
    /// treating absence as a benign answer.
    pub fn get(&self, path: &str) -> Option<&MergeRecord> {
        self.records.get(path)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &MergeRecord)> {
        self.records
            .iter()
            .map(|(path, record)| (path.as_str(), record))
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// True when at least one record did not fit under the cap.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Bytes currently retained by path and source text.
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }

    fn mark_truncated(&mut self) {
        self.truncated = true;
    }

    /// Record one merge outcome. A weaker source never demotes a winner;
    /// at equal priority the first source stands (the values agreed). The
    /// byte cap applies before any text is retained, so a hostile path or
    /// label can only mark the map truncated.
    fn record(&mut self, path: &str, winner: &MergeSource, loser: Option<&MergeSource>) {
        let winner = winner.clone().bounded();
        let loser = loser.cloned().map(MergeSource::bounded);
        if let Some(mut record) = self.records.get(path).cloned() {
            match winner.priority.cmp(&record.winner.priority) {
                std::cmp::Ordering::Greater => {
                    if self.try_charge(source_bytes(&winner), source_bytes(&record.winner)) {
                        record.losers.push(record.winner.clone());
                    }
                    record.winner = winner;
                }
                std::cmp::Ordering::Less => {
                    // A weaker source never demotes the winner.
                    if self.try_charge(source_bytes(&winner), 0) {
                        record.losers.push(winner);
                    }
                }
                std::cmp::Ordering::Equal => {
                    // Same priority and (by construction) agreeing values:
                    // keep the first winner; agreement is not a defeat.
                }
            }
            if let Some(loser) = loser {
                if record.losers.len() < MAX_LOSERS && self.try_charge(source_bytes(&loser), 0) {
                    record.losers.push(loser);
                } else if record.losers.len() >= MAX_LOSERS {
                    record.losers_truncated = true;
                }
            }
            record.losers.sort();
            record.losers.dedup();
            record.losers.retain(|source| *source != record.winner);
            if record.losers.len() > MAX_LOSERS {
                record.losers.truncate(MAX_LOSERS);
                record.losers_truncated = true;
            }
            self.records.insert(path.to_string(), record);
            return;
        }
        let mut losers: Vec<MergeSource> = loser.into_iter().collect();
        losers.retain(|source| *source != winner);
        let cost = record_bytes(path, &winner, &losers);
        if self.records.len() >= self.limit || !self.try_charge(cost, 0) {
            self.truncated = true;
            return;
        }
        self.records.insert(
            path.to_string(),
            MergeRecord {
                winner,
                losers,
                losers_truncated: false,
            },
        );
    }

    /// Retain `addition` bytes when the map stays under its byte cap.
    /// `replaced` is text the caller is dropping in the same step.
    fn try_charge(&mut self, addition: usize, replaced: usize) -> bool {
        let next = self.bytes.saturating_sub(replaced).saturating_add(addition);
        if next > self.byte_limit {
            self.truncated = true;
            return false;
        }
        self.bytes = next;
        true
    }

    /// Drop every record at or below `path`. A successful merge replaces
    /// that subtree, so a record for a path the output no longer contains
    /// would be wrong data.
    fn prune_subtree(&mut self, path: &str) {
        if path.is_empty() {
            self.records.clear();
            self.bytes = 0;
            return;
        }
        let prefix = format!("{path}.");
        let stale: Vec<String> = self
            .records
            .keys()
            .filter(|candidate| candidate.as_str() == path || candidate.starts_with(&prefix))
            .cloned()
            .collect();
        for key in stale {
            if let Some(record) = self.records.remove(&key) {
                self.bytes =
                    self.bytes
                        .saturating_sub(record_bytes(&key, &record.winner, &record.losers));
            }
        }
    }

    /// Merge another map into this one, preserving the stronger winner
    /// and the union of losers.
    fn extend(&mut self, other: MergeProvenance) {
        if other.truncated {
            self.truncated = true;
        }
        for (path, record) in other.records {
            self.record(&path, &record.winner, None);
            for loser in &record.losers {
                self.record(&path, &record.winner, Some(loser));
            }
            if record.losers_truncated {
                if let Some(stored) = self.records.get_mut(&path) {
                    stored.losers_truncated = true;
                }
            }
        }
    }
}

fn source_bytes(source: &MergeSource) -> usize {
    source.file.as_ref().map_or(0, String::len) + source.label.len() + 32
}

fn record_bytes(path: &str, winner: &MergeSource, losers: &[MergeSource]) -> usize {
    path.len() + source_bytes(winner) + losers.iter().map(source_bytes).sum::<usize>()
}

/// Two sources disagree at the same priority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeConflict {
    pub path: String,
    pub left: MergeSource,
    pub right: MergeSource,
}

impl MergeConflict {
    /// Stable diagnostic code.
    pub fn code(&self) -> &'static str {
        "E046"
    }

    /// A value-free E046 message: the path and both source locations.
    /// Labels are source-controlled, so control characters are escaped.
    pub fn message(&self) -> String {
        format!(
            "merge conflict at '{}': {} and {} disagree",
            escape_text(if self.path.is_empty() {
                "<root>"
            } else {
                self.path.as_str()
            }),
            describe_source(&self.left),
            describe_source(&self.right),
        )
    }
}

fn describe_source(source: &MergeSource) -> String {
    // Identities keep their case: a path on a case-sensitive filesystem
    // is not interchangeable with a lowercased spelling. Only the
    // priority token is normalized.
    let location = match (&source.file, source.line) {
        (Some(file), line) if line > 0 => {
            format!("{}:{}", quoted(file), line)
        }
        (Some(file), _) => quoted(file),
        (None, line) if line > 0 => format!("line {line}"),
        (None, _) => String::new(),
    };
    let label = quoted(&source.label);
    let priority = match source.priority {
        MergePriority::Default => "default",
        MergePriority::Include => "include",
        MergePriority::Local => "local",
        MergePriority::Profile => "profile",
    };
    if location.is_empty() {
        format!("{label} ({priority})")
    } else {
        format!("{label} ({location}, {priority})")
    }
}

/// Render source-controlled text with delimiters escaped, so a label
/// cannot spoof the structured `label (file:line, priority)` form. Plain
/// paths stay readable; only text containing a delimiter, quote,
/// backslash, or control character is quoted.
fn quoted(text: &str) -> String {
    let needs_quotes = text
        .chars()
        .any(|character| matches!(character, '"' | '\\' | '(' | ')' | ','));
    if !needs_quotes {
        // Control characters still escape, so a newline cannot inject a
        // forged line into a diagnostic or a log.
        return text.chars().flat_map(|c| c.escape_default()).collect();
    }
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '(' | ')' | ',' => {
                out.push('\\');
                out.push(character);
            }
            other => out.extend(other.escape_default()),
        }
    }
    out.push('"');
    out
}

fn escape_text(text: &str) -> String {
    text.chars()
        .flat_map(|character| character.escape_default())
        .collect()
}

/// The merge budget tripped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeBudgetKind {
    Depth,
    Nodes,
    Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeBudget {
    pub kind: MergeBudgetKind,
    pub path: String,
    pub limit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeError {
    Conflict(Box<MergeConflict>),
    Budget(MergeBudget),
}

impl MergeError {
    pub fn code(&self) -> &'static str {
        match self {
            MergeError::Conflict(_) => "E046",
            MergeError::Budget(budget) => match budget.kind {
                MergeBudgetKind::Depth => "X003",
                MergeBudgetKind::Nodes => "X001",
                MergeBudgetKind::Bytes => "X006",
            },
        }
    }

    pub fn message(&self) -> String {
        match self {
            MergeError::Conflict(conflict) => conflict.message(),
            MergeError::Budget(budget) => format!(
                "merge budget exceeded at '{}': {:?} limit {}",
                escape_text(&budget.path),
                budget.kind,
                budget.limit
            ),
        }
    }

    /// A stable diagnostic for this failure. Conflict messages never echo
    /// values; budget messages name only the path and limit.
    pub fn to_diagnostic(&self) -> crate::diag::Diagnostic {
        crate::diag::Diagnostic {
            severity: crate::diag::Severity::Error,
            code: self.code().to_string(),
            message: self.message(),
            path: match self {
                MergeError::Conflict(conflict) if !conflict.path.is_empty() => {
                    Some(conflict.path.clone())
                }
                MergeError::Budget(budget) if !budget.path.is_empty() => Some(budget.path.clone()),
                _ => None,
            },
            span: None,
            repair: None,
            confidence: 1.0,
            file: None,
        }
    }
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for MergeError {}

impl From<Box<MergeConflict>> for MergeError {
    fn from(conflict: Box<MergeConflict>) -> Self {
        MergeError::Conflict(conflict)
    }
}

/// Limits for one merge. The default matches the shipped evaluator
/// ceilings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeLimits {
    pub max_depth: usize,
    pub max_nodes: u64,
    pub max_bytes: u64,
}

impl Default for MergeLimits {
    fn default() -> Self {
        Self {
            max_depth: MAX_MERGE_DEPTH,
            max_nodes: MAX_MERGE_NODES,
            max_bytes: MAX_MERGE_BYTES,
        }
    }
}

struct MergeState<'a> {
    depth: usize,
    limits: &'a MergeLimits,
}

impl MergeState<'_> {
    /// Enter one table level. Input depth, node count, and bytes are
    /// checked up front by [`Precheck`]; the recursive walk only needs the
    /// nesting guard for its own calls.
    fn enter(&mut self, path: &str) -> Result<(), MergeError> {
        if self.depth > self.limits.max_depth {
            return Err(MergeError::Budget(MergeBudget {
                kind: MergeBudgetKind::Depth,
                path: path.to_string(),
                limit: self.limits.max_depth as u64,
            }));
        }
        Ok(())
    }
}

/// Segment of a reconstructed path, used only on a budget failure.
#[derive(Clone, Copy)]
enum PathSegment<'a> {
    Key(&'a str),
    Index(usize),
}

struct PreFrame<'a> {
    value: &'a Value,
    depth: usize,
    parent: Option<usize>,
    segment: PathSegment<'a>,
}

/// Iterative depth/node/byte walk over one input tree. It runs before
/// any clone or comparison, so a hostile deeply nested or huge value is
/// refused instead of aborting the process in `clone` or equality.
struct Precheck<'a> {
    limits: &'a MergeLimits,
    nodes: u64,
    bytes: u64,
}

impl Precheck<'_> {
    fn walk(&mut self, value: &Value, path: &str) -> Result<(), MergeError> {
        let mut frames: Vec<PreFrame<'_>> = vec![PreFrame {
            value,
            depth: 0,
            parent: None,
            segment: PathSegment::Key(""),
        }];
        let mut stack = vec![0usize];
        while let Some(index) = stack.pop() {
            let frame = &frames[index];
            let value = frame.value;
            let depth = frame.depth;
            let path = if frame.parent.is_none() {
                path.to_string()
            } else {
                reconstruct_path(&frames, index)
            };
            if depth > self.limits.max_depth {
                return Err(MergeError::Budget(MergeBudget {
                    kind: MergeBudgetKind::Depth,
                    path,
                    limit: self.limits.max_depth as u64,
                }));
            }
            self.nodes = self.nodes.saturating_add(1);
            if self.nodes > self.limits.max_nodes {
                return Err(MergeError::Budget(MergeBudget {
                    kind: MergeBudgetKind::Nodes,
                    path,
                    limit: self.limits.max_nodes,
                }));
            }
            let cost = match value {
                Value::String(text) => text.len() as u64,
                // Secret length is material: charge a constant so budget
                // outcomes cannot binary-search it.
                Value::Secret(_) => 1,
                Value::Blob {
                    payload,
                    media_type,
                    ..
                } => 1u64
                    .saturating_add(payload.as_ref().map_or(0, |p| p.len() as u64))
                    .saturating_add(media_type.len() as u64),
                Value::Array(items) => {
                    for (offset, item) in items.iter().enumerate() {
                        stack.push(frames.len());
                        frames.push(PreFrame {
                            value: item,
                            depth: depth.saturating_add(1),
                            parent: Some(index),
                            segment: PathSegment::Index(offset),
                        });
                    }
                    1
                }
                Value::Table(map) => {
                    let mut keys = 0u64;
                    for (key, item) in map {
                        keys = keys.saturating_add(key.len() as u64);
                        stack.push(frames.len());
                        frames.push(PreFrame {
                            value: item,
                            depth: depth.saturating_add(1),
                            parent: Some(index),
                            segment: PathSegment::Key(key),
                        });
                    }
                    1u64.saturating_add(keys)
                }
                _ => 1,
            };
            self.bytes = self.bytes.saturating_add(cost);
            if self.bytes > self.limits.max_bytes {
                return Err(MergeError::Budget(MergeBudget {
                    kind: MergeBudgetKind::Bytes,
                    path,
                    limit: self.limits.max_bytes,
                }));
            }
        }
        Ok(())
    }
}

fn reconstruct_path(frames: &[PreFrame<'_>], mut index: usize) -> String {
    let mut segments: Vec<String> = Vec::new();
    while let Some(parent) = frames[index].parent {
        segments.push(match frames[index].segment {
            PathSegment::Key(key) => key.to_string(),
            PathSegment::Index(offset) => format!("[{offset}]"),
        });
        index = parent;
    }
    segments.reverse();
    let mut out = String::new();
    for segment in segments {
        if segment.starts_with('[') {
            out.push_str(&segment);
        } else {
            if !out.is_empty() {
                out.push('.');
            }
            out.push_str(&segment);
        }
    }
    out
}

/// Unify two values at `path` with the shipped limits. Provenance is
/// committed only when the merge succeeds.
///
/// The limits bound one call. A host with a cumulative budget (the
/// evaluator's X006 accountant) must charge the returned value's measured
/// size to that accountant; include wiring does this in L-P09 phase 2.
pub fn unify(
    left: &Value,
    left_source: &MergeSource,
    right: &Value,
    right_source: &MergeSource,
    path: &str,
    provenance: &mut MergeProvenance,
) -> Result<Value, MergeError> {
    unify_with_limits(
        left,
        left_source,
        right,
        right_source,
        path,
        provenance,
        &MergeLimits::default(),
    )
}

/// Resilient merge: an equal-priority conflict keeps the first source's
/// value (source order) and returns the conflict for an E046 diagnostic.
/// Budget failures stay fatal.
pub fn unify_or_left(
    left: &Value,
    left_source: &MergeSource,
    right: &Value,
    right_source: &MergeSource,
    path: &str,
    provenance: &mut MergeProvenance,
) -> Result<(Value, Option<Box<MergeConflict>>), MergeError> {
    match unify(left, left_source, right, right_source, path, provenance) {
        Ok(value) => Ok((value, None)),
        Err(MergeError::Conflict(conflict)) => Ok((left.clone(), Some(conflict))),
        Err(error) => Err(error),
    }
}

/// Unify with explicit limits. On `Err`, `provenance` is unchanged.
pub fn unify_with_limits(
    left: &Value,
    left_source: &MergeSource,
    right: &Value,
    right_source: &MergeSource,
    path: &str,
    provenance: &mut MergeProvenance,
    limits: &MergeLimits,
) -> Result<Value, MergeError> {
    // Refuse hostile shapes before any clone or equality walk: both
    // inputs are charged against one shared depth/node/byte ceiling.
    let mut precheck = Precheck {
        limits,
        nodes: 0,
        bytes: 0,
    };
    precheck.walk(left, path)?;
    precheck.walk(right, path)?;
    let mut local = MergeProvenance::with_limits(provenance.limit, provenance.byte_limit);
    let mut state = MergeState { depth: 0, limits };
    let value = unify_inner(
        left,
        left_source,
        right,
        right_source,
        path,
        &mut local,
        &mut state,
    )?;
    if local.truncated() {
        provenance.mark_truncated();
    }
    // A successful merge replaces the subtree at `path`; records below it
    // describe values the output no longer contains.
    provenance.prune_subtree(path);
    provenance.extend(local);
    Ok(value)
}

fn unify_inner(
    left: &Value,
    left_source: &MergeSource,
    right: &Value,
    right_source: &MergeSource,
    path: &str,
    provenance: &mut MergeProvenance,
    state: &mut MergeState<'_>,
) -> Result<Value, MergeError> {
    state.enter(path)?;
    if let (Value::Table(left_table), Value::Table(right_table)) = (left, right) {
        let mut merged = left_table.clone();
        for (key, right_value) in right_table {
            let child = join(path, key);
            match merged.get(key) {
                Some(left_value) => {
                    state.depth += 1;
                    let value = unify_inner(
                        left_value,
                        left_source,
                        right_value,
                        right_source,
                        &child,
                        provenance,
                        state,
                    )?;
                    state.depth -= 1;
                    merged.insert(key.clone(), value);
                }
                None => {
                    state.depth += 1;
                    record_leaves(right_value, right_source, &child, provenance, state)?;
                    state.depth -= 1;
                    merged.insert(key.clone(), right_value.clone());
                }
            }
        }
        for (key, left_value) in left_table {
            if !right_table.contains_key(key) {
                let child = join(path, key);
                state.depth += 1;
                record_leaves(left_value, left_source, &child, provenance, state)?;
                state.depth -= 1;
            }
        }
        // The table path belongs to a side that actually contributed keys:
        // an empty stronger table inherits the lower table and must not
        // claim authorship of it.
        let table_winner = match (left_table.is_empty(), right_table.is_empty()) {
            (true, true) => left_source,
            (true, false) => right_source,
            (false, true) => left_source,
            (false, false) => {
                if right_source.priority > left_source.priority {
                    right_source
                } else {
                    left_source
                }
            }
        };
        provenance.record(path, table_winner, None);
        return Ok(Value::Table(merged));
    }
    if merge_agree(left, right) {
        // Agreement records the stronger side, never a weaker left just
        // because it was first.
        let winner = if right_source.priority > left_source.priority {
            right_source
        } else {
            left_source
        };
        provenance.record(path, winner, None);
        return Ok(left.clone());
    }
    if left_source.priority == right_source.priority {
        return Err(MergeError::Conflict(Box::new(MergeConflict {
            path: path.to_string(),
            left: left_source.clone(),
            right: right_source.clone(),
        })));
    }
    let (winner, winner_source, loser) = if left_source.priority > right_source.priority {
        (left, left_source, right_source)
    } else {
        (right, right_source, left_source)
    };
    provenance.record(path, winner_source, Some(loser));
    Ok(winner.clone())
}

/// Raw-tree equality for a merge. Material is never compared: a secret
/// and a tainted blob are unequal to everything, including themselves.
/// Blob identity is hash, size, media type, and taint; the retained
/// payload is a materialization cache, not identity. Callers pass
/// evaluated (raw) trees; redacted trees would defeat the material guard.
fn merge_agree(left: &Value, right: &Value) -> bool {
    let mut stack = vec![(left, right)];
    while let Some((left, right)) = stack.pop() {
        match (left, right) {
            (Value::Null, Value::Null) => {}
            (Value::Bool(x), Value::Bool(y)) if x == y => {}
            (Value::Int(x), Value::Int(y)) if x == y => {}
            (Value::Float(x), Value::Float(y)) if x == y || (x.is_nan() && y.is_nan()) => {}
            (Value::String(x), Value::String(y)) if x == y => {}
            (Value::DurationMs(x), Value::DurationMs(y)) if x == y => {}
            (Value::Quantity(x, ux), Value::Quantity(y, uy))
                if (x == y || (x.is_nan() && y.is_nan())) && ux == uy => {}
            (
                Value::Blob {
                    hash: xh,
                    size: xs,
                    tainted: xt,
                    media_type: xm,
                    ..
                },
                Value::Blob {
                    hash: yh,
                    size: ys,
                    tainted: yt,
                    media_type: ym,
                    ..
                },
            ) if !xt && !yt && xh == yh && xs == ys && xm == ym => {}
            (Value::Array(xs), Value::Array(ys)) if xs.len() == ys.len() => {
                stack.extend(xs.iter().zip(ys));
            }
            (Value::Table(xs), Value::Table(ys)) if xs.len() == ys.len() => {
                for (key, x) in xs {
                    let Some(y) = ys.get(key) else {
                        return false;
                    };
                    stack.push((x, y));
                }
            }
            // Includes Secret/Secret: material equality is never reported.
            _ => return false,
        }
    }
    true
}

fn record_leaves(
    value: &Value,
    source: &MergeSource,
    path: &str,
    provenance: &mut MergeProvenance,
    state: &mut MergeState<'_>,
) -> Result<(), MergeError> {
    state.enter(path)?;
    match value {
        Value::Table(map) if !map.is_empty() => {
            for (key, child) in map {
                let child_path = join(path, key);
                state.depth += 1;
                record_leaves(child, source, &child_path, provenance, state)?;
                state.depth -= 1;
            }
        }
        _ => provenance.record(path, source, None),
    }
    Ok(())
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Value;
    use indexmap::IndexMap;

    fn table(pairs: &[(&str, Value)]) -> Value {
        let mut map = IndexMap::new();
        for (key, value) in pairs {
            map.insert((*key).to_string(), value.clone());
        }
        Value::Table(map)
    }

    fn source(priority: MergePriority, label: &str) -> MergeSource {
        MergeSource::new(priority, None, 1, label)
    }

    fn expect_conflict(result: Result<Value, MergeError>) -> Box<MergeConflict> {
        match result {
            Err(MergeError::Conflict(conflict)) => conflict,
            other => panic!("expected a conflict, got {other:?}"),
        }
    }

    #[test]
    fn scalar_priority_pairs_decide_and_equal_priority_conflicts() {
        let left = Value::Int(1);
        let right = Value::Int(2);
        for (low, high) in [
            (MergePriority::Default, MergePriority::Include),
            (MergePriority::Include, MergePriority::Local),
            (MergePriority::Local, MergePriority::Profile),
            (MergePriority::Default, MergePriority::Profile),
        ] {
            let mut provenance = MergeProvenance::new();
            let result = unify(
                &left,
                &source(low, "low"),
                &right,
                &source(high, "high"),
                "port",
                &mut provenance,
            )
            .expect("higher priority wins");
            assert!(crate::diff::same_value(&result, &right));
            let record = provenance.get("port").expect("record");
            assert_eq!(record.winner.label, "high");
            assert_eq!(record.losers[0].label, "low");
        }
        let mut provenance = MergeProvenance::new();
        let conflict = expect_conflict(unify(
            &left,
            &source(MergePriority::Local, "a"),
            &right,
            &source(MergePriority::Local, "b"),
            "port",
            &mut provenance,
        ));
        assert_eq!(conflict.path, "port");
        assert_eq!(conflict.code(), "E046");
        assert!(conflict.message().contains("merge conflict"));
        assert!(conflict.message().contains("port"));
    }

    #[test]
    fn equal_values_agree_at_equal_priority() {
        let value = Value::String("x".into());
        let mut provenance = MergeProvenance::new();
        let result = unify(
            &value,
            &source(MergePriority::Local, "a"),
            &value,
            &source(MergePriority::Local, "b"),
            "",
            &mut provenance,
        )
        .expect("equal values do not conflict");
        assert!(crate::diff::same_value(&result, &value));
        assert!(provenance.get("").expect("record").losers.is_empty());
    }

    #[test]
    fn tables_merge_recursively_and_keep_both_sides() {
        let base = table(&[
            ("host", Value::String("h".into())),
            (
                "server",
                table(&[("port", Value::Int(80)), ("tls", Value::Bool(true))]),
            ),
        ]);
        let overlay = table(&[
            ("server", table(&[("port", Value::Int(443))])),
            ("extra", Value::Int(1)),
        ]);
        let mut provenance = MergeProvenance::new();
        let merged = unify(
            &base,
            &source(MergePriority::Include, "base"),
            &overlay,
            &source(MergePriority::Profile, "prod"),
            "",
            &mut provenance,
        )
        .expect("tables merge");
        let expected = table(&[
            ("host", Value::String("h".into())),
            (
                "server",
                table(&[("port", Value::Int(443)), ("tls", Value::Bool(true))]),
            ),
            ("extra", Value::Int(1)),
        ]);
        assert!(
            crate::diff::same_value(&merged, &expected),
            "merged: {merged:?}"
        );
        assert_eq!(
            provenance.get("server.port").expect("port").winner.label,
            "prod"
        );
        assert_eq!(
            provenance.get("server.tls").expect("tls").winner.label,
            "base"
        );
        assert_eq!(provenance.get("host").expect("host").winner.label, "base");
        assert_eq!(provenance.get("extra").expect("extra").winner.label, "prod");
        assert_eq!(
            provenance.get("server").expect("table").winner.label,
            "prod"
        );
    }

    #[test]
    fn table_versus_scalar_follows_priority() {
        let table = table(&[("a", Value::Int(1))]);
        let scalar = Value::Int(2);
        let mut provenance = MergeProvenance::new();
        let conflict = expect_conflict(unify(
            &table,
            &source(MergePriority::Local, "a"),
            &scalar,
            &source(MergePriority::Local, "b"),
            "x",
            &mut provenance,
        ));
        assert_eq!(conflict.path, "x");
        let winner = unify(
            &table,
            &source(MergePriority::Include, "inc"),
            &scalar,
            &source(MergePriority::Local, "local"),
            "x",
            &mut provenance,
        )
        .expect("local wins");
        assert!(crate::diff::same_value(&winner, &scalar));
    }

    #[test]
    fn arrays_compare_semantically_and_follow_priority() {
        let left = Value::Array(vec![Value::Int(1), Value::Int(2)]);
        let equal = Value::Array(vec![Value::Int(1), Value::Int(2)]);
        let different = Value::Array(vec![Value::Int(1), Value::Int(3)]);
        let mut provenance = MergeProvenance::new();
        let equal_result = unify(
            &left,
            &source(MergePriority::Local, "a"),
            &equal,
            &source(MergePriority::Local, "b"),
            "items",
            &mut provenance,
        )
        .expect("equal arrays agree");
        assert!(crate::diff::same_value(&equal_result, &left));
        let conflict = expect_conflict(unify(
            &left,
            &source(MergePriority::Local, "a"),
            &different,
            &source(MergePriority::Local, "b"),
            "items",
            &mut provenance,
        ));
        assert_eq!(conflict.path, "items");
        let profile_result = unify(
            &left,
            &source(MergePriority::Include, "inc"),
            &different,
            &source(MergePriority::Profile, "profile"),
            "items",
            &mut provenance,
        )
        .expect("profile wins");
        assert!(crate::diff::same_value(&profile_result, &different));
    }

    #[test]
    fn secrets_are_never_compared_or_merged_by_material() {
        let a = Value::Secret(zeroize::Zeroizing::new("hunter2".into()));
        let b = Value::Secret(zeroize::Zeroizing::new("hunter2".into()));
        let c = Value::Secret(zeroize::Zeroizing::new("other".into()));
        // Equal material from two sources is still a conflict at equal
        // priority: the merge never reads the bytes.
        let mut provenance = MergeProvenance::new();
        let conflict = expect_conflict(unify(
            &a,
            &source(MergePriority::Local, "a"),
            &b,
            &source(MergePriority::Local, "b"),
            "token",
            &mut provenance,
        ));
        assert!(!format!("{conflict:?}").contains("hunter2"));
        assert!(!conflict.message().contains("hunter2"));
        // Priority alone decides without touching material.
        let winner = unify(
            &a,
            &source(MergePriority::Include, "inc"),
            &c,
            &source(MergePriority::Local, "local"),
            "token",
            &mut provenance,
        )
        .expect("priority wins");
        assert!(matches!(winner, Value::Secret(_)));
        // Arrays of secrets are incomparable element-wise.
        let array_a = Value::Array(vec![a.clone()]);
        let array_b = Value::Array(vec![b]);
        let conflict = expect_conflict(unify(
            &array_a,
            &source(MergePriority::Local, "a"),
            &array_b,
            &source(MergePriority::Local, "b"),
            "tokens",
            &mut provenance,
        ));
        assert_eq!(conflict.path, "tokens");
    }

    #[test]
    fn tainted_blobs_are_incomparable_and_untainted_blobs_compare_identity() {
        let tainted = Value::Blob {
            hash: [1u8; 32],
            size: 3,
            tainted: true,
            media_type: "text/plain".into(),
            payload: None,
        };
        let mut provenance = MergeProvenance::new();
        expect_conflict(unify(
            &tainted,
            &source(MergePriority::Local, "a"),
            &tainted,
            &source(MergePriority::Local, "b"),
            "blob",
            &mut provenance,
        ));
        let clean = Value::Blob {
            hash: [2u8; 32],
            size: 3,
            tainted: false,
            media_type: "text/plain".into(),
            payload: None,
        };
        let merged = unify(
            &clean,
            &source(MergePriority::Local, "a"),
            &clean,
            &source(MergePriority::Local, "b"),
            "blob",
            &mut provenance,
        )
        .expect("untainted identities agree");
        assert!(matches!(merged, Value::Blob { .. }));
    }

    #[test]
    fn provenance_is_bounded_and_reports_truncation() {
        let mut provenance = MergeProvenance::with_limit(2);
        for index in 0..3 {
            provenance.record(
                &format!("k{index}"),
                &source(MergePriority::Local, "a"),
                None,
            );
        }
        assert_eq!(provenance.len(), 2);
        assert!(provenance.truncated());
    }

    #[test]
    fn a_failed_merge_leaves_provenance_unchanged() {
        let left = table(&[
            ("ok", table(&[("k", Value::Int(1))])),
            ("bad", Value::Int(1)),
        ]);
        let right = table(&[
            ("ok", table(&[("k", Value::Int(1))])),
            ("bad", Value::Int(2)),
        ]);
        let mut provenance = MergeProvenance::new();
        expect_conflict(unify(
            &left,
            &source(MergePriority::Local, "a"),
            &right,
            &source(MergePriority::Local, "b"),
            "",
            &mut provenance,
        ));
        assert!(provenance.is_empty(), "{provenance:?}");
    }

    #[test]
    fn provenance_keeps_the_strongest_winner_and_bounds_losers() {
        let mut provenance = MergeProvenance::new();
        let path = "x";
        provenance.record(path, &source(MergePriority::Profile, "profile"), None);
        provenance.record(path, &source(MergePriority::Local, "local"), None);
        assert_eq!(
            provenance.get(path).expect("record").winner.label,
            "profile"
        );
        for index in 0..40 {
            provenance.record(
                path,
                &source(MergePriority::Include, &format!("inc{index}")),
                None,
            );
        }
        let record = provenance.get(path).expect("record");
        assert_eq!(record.winner.label, "profile");
        assert!(record.losers.len() <= MAX_LOSERS);
        assert!(record.losers_truncated);
    }

    #[test]
    fn recursion_and_clone_budgets_fail_closed() {
        fn nested(depth: usize) -> Value {
            let mut value = Value::Int(1);
            for index in 0..depth {
                value = table(&[(&format!("k{index}"), value)]);
            }
            value
        }
        let deep = nested(300);
        let mut provenance = MergeProvenance::new();
        let error = unify(
            &deep,
            &source(MergePriority::Local, "a"),
            &deep,
            &source(MergePriority::Local, "b"),
            "",
            &mut provenance,
        )
        .expect_err("depth budget");
        assert_eq!(error.code(), "X003");
        assert!(provenance.is_empty());

        let mut provenance = MergeProvenance::new();
        let limits = MergeLimits {
            max_depth: MAX_MERGE_DEPTH,
            max_nodes: 2,
            max_bytes: MAX_MERGE_BYTES,
        };
        let small = table(&[("a", Value::Int(1)), ("b", Value::Int(2))]);
        let error = unify_with_limits(
            &small,
            &source(MergePriority::Local, "a"),
            &small,
            &source(MergePriority::Local, "b"),
            "",
            &mut provenance,
            &limits,
        )
        .expect_err("node budget");
        assert_eq!(error.code(), "X001");

        let mut provenance = MergeProvenance::new();
        let limits = MergeLimits {
            max_depth: MAX_MERGE_DEPTH,
            max_nodes: MAX_MERGE_NODES,
            max_bytes: 1,
        };
        let words = table(&[("a", Value::String("long".into()))]);
        let error = unify_with_limits(
            &words,
            &source(MergePriority::Local, "a"),
            &words,
            &source(MergePriority::Local, "b"),
            "",
            &mut provenance,
            &limits,
        )
        .expect_err("byte budget");
        assert_eq!(error.code(), "X006");
    }

    #[test]
    fn conflict_messages_name_both_locations_and_escape_labels() {
        let left = MergeSource::new(
            MergePriority::Include,
            Some("lib\nx.active".into()),
            7,
            "include \"lib\nx.active\"",
        );
        let right = MergeSource::new(
            MergePriority::Include,
            Some("b.active".into()),
            9,
            "include \"b.active\"",
        );
        let conflict = MergeConflict {
            path: "server.port".into(),
            left,
            right,
        };
        let message = conflict.message();
        assert!(message.contains("lib\\nx.active:7"), "{message}");
        assert!(message.contains("b.active:9"), "{message}");
        assert!(!message.contains('\n'), "{message}");
    }

    #[test]
    fn an_empty_higher_priority_table_inherits_lower_keys() {
        let base = table(&[("a", Value::Int(1)), ("b", Value::Int(2))]);
        let empty = table(&[]);
        let mut provenance = MergeProvenance::new();
        let merged = unify(
            &base,
            &source(MergePriority::Local, "local"),
            &empty,
            &source(MergePriority::Profile, "lockdown"),
            "",
            &mut provenance,
        )
        .expect("table union");
        assert!(crate::diff::same_value(&merged, &base));
        // The empty stronger table contributed nothing, so it must not
        // claim authorship of the inherited root path.
        let record = provenance.get("").expect("root record");
        assert_eq!(record.winner.label, "local");
    }

    #[test]
    fn deep_tables_and_arrays_are_refused_before_any_clone() {
        fn nested_table(depth: usize) -> Value {
            let mut value = Value::Int(1);
            for index in 0..depth {
                value = table(&[(&format!("k{index}"), value)]);
            }
            value
        }
        fn nested_array(depth: usize) -> Value {
            let mut value = Value::Int(1);
            for _ in 0..depth {
                value = Value::Array(vec![value]);
            }
            value
        }
        let mut provenance = MergeProvenance::new();
        let error = unify(
            &nested_table(MAX_MERGE_DEPTH + 2),
            &source(MergePriority::Local, "a"),
            &nested_table(MAX_MERGE_DEPTH + 2),
            &source(MergePriority::Local, "b"),
            "deep",
            &mut provenance,
        )
        .expect_err("depth");
        assert_eq!(error.code(), "X003");
        let error = unify(
            &nested_array(MAX_MERGE_DEPTH + 2),
            &source(MergePriority::Local, "a"),
            &nested_array(MAX_MERGE_DEPTH + 2),
            &source(MergePriority::Local, "b"),
            "deep",
            &mut provenance,
        )
        .expect_err("array depth");
        assert_eq!(error.code(), "X003");
    }

    #[test]
    fn array_content_and_left_only_keys_are_charged() {
        let array = Value::Array(vec![Value::String("x".repeat(64))]);
        let mut provenance = MergeProvenance::new();
        let limits = MergeLimits {
            max_depth: MAX_MERGE_DEPTH,
            max_nodes: MAX_MERGE_NODES,
            max_bytes: 16,
        };
        let error = unify_with_limits(
            &array,
            &source(MergePriority::Local, "a"),
            &array,
            &source(MergePriority::Local, "b"),
            "items",
            &mut provenance,
            &limits,
        )
        .expect_err("array bytes");
        assert_eq!(error.code(), "X006");

        let key = "k".repeat(40);
        let left = table(&[(key.as_str(), Value::Int(1))]);
        let mut provenance = MergeProvenance::new();
        let error = unify_with_limits(
            &left,
            &source(MergePriority::Local, "a"),
            &table(&[]),
            &source(MergePriority::Local, "b"),
            "",
            &mut provenance,
            &limits,
        )
        .expect_err("left key bytes");
        assert_eq!(error.code(), "X006");
    }

    #[test]
    fn blob_payload_bytes_are_charged() {
        let blob = Value::Blob {
            hash: [3u8; 32],
            size: 4,
            tainted: false,
            media_type: "text/plain".into(),
            payload: Some(vec![0u8; 64]),
        };
        let mut provenance = MergeProvenance::new();
        let limits = MergeLimits {
            max_depth: MAX_MERGE_DEPTH,
            max_nodes: MAX_MERGE_NODES,
            max_bytes: 32,
        };
        let error = unify_with_limits(
            &blob,
            &source(MergePriority::Local, "a"),
            &blob,
            &source(MergePriority::Local, "b"),
            "blob",
            &mut provenance,
            &limits,
        )
        .expect_err("payload bytes");
        assert_eq!(error.code(), "X006");
    }

    #[test]
    fn budget_outcomes_do_not_reveal_secret_length() {
        let mut results = Vec::new();
        for length in [1usize, 5, 100, 10_000] {
            let secret = Value::Secret(zeroize::Zeroizing::new("x".repeat(length)));
            let mut provenance = MergeProvenance::new();
            let limits = MergeLimits {
                max_depth: MAX_MERGE_DEPTH,
                max_nodes: MAX_MERGE_NODES,
                max_bytes: 10,
            };
            results.push(unify_with_limits(
                &secret,
                &source(MergePriority::Local, "a"),
                &secret,
                &source(MergePriority::Local, "b"),
                "token",
                &mut provenance,
                &limits,
            ));
        }
        // Same shape for every length: a conflict (secrets never agree),
        // never a byte-budget difference.
        for result in results {
            match result {
                Err(MergeError::Conflict(conflict)) => assert_eq!(conflict.path, "token"),
                other => panic!("expected the same conflict for every length, got {other:?}"),
            }
        }
    }

    #[test]
    fn agreement_records_the_stronger_source() {
        let value = Value::Int(7);
        let mut provenance = MergeProvenance::new();
        unify(
            &value,
            &source(MergePriority::Include, "inc"),
            &value,
            &source(MergePriority::Profile, "prod"),
            "port",
            &mut provenance,
        )
        .expect("agree");
        let record = provenance.get("port").expect("record");
        assert_eq!(record.winner.priority, MergePriority::Profile);
        assert_eq!(record.winner.label, "prod");
    }

    #[test]
    fn a_successful_merge_prunes_stale_descendants() {
        let mut provenance = MergeProvenance::new();
        provenance.record("x.a", &source(MergePriority::Include, "inc"), None);
        let merged = unify(
            &table(&[("a", Value::Int(1))]),
            &source(MergePriority::Include, "inc"),
            &Value::Int(5),
            &source(MergePriority::Profile, "prod"),
            "x",
            &mut provenance,
        )
        .expect("higher priority scalar wins");
        assert!(matches!(merged, Value::Int(5)));
        assert!(
            provenance.get("x.a").is_none(),
            "stale descendant kept: {provenance:?}"
        );
        assert!(provenance.get("x").is_some());
    }

    #[test]
    fn provenance_bytes_are_bounded() {
        let mut provenance = MergeProvenance::with_limits(65_536, 128);
        for index in 0..64 {
            provenance.record(
                &format!("{index}-{}", "p".repeat(32)),
                &source(MergePriority::Local, "local-source"),
                None,
            );
        }
        assert!(provenance.truncated());
        assert!(provenance.retained_bytes() <= 128);
        assert!(provenance.len() < 64);
    }

    #[test]
    fn identities_keep_case_and_resist_spoofing() {
        let spoof = MergeSource::new(
            MergePriority::Include,
            Some("Config.Active".into()),
            4,
            "evil, local) (x",
        );
        let text = describe_source(&spoof);
        assert!(text.contains("Config.Active"), "{text}");
        assert!(!text.contains("local) (x"), "{text}");
        assert!(text.contains("include"), "{text}");
    }

    #[test]
    fn unify_or_left_keeps_source_order_and_reports_the_conflict() {
        let mut provenance = MergeProvenance::new();
        let (value, conflict) = unify_or_left(
            &Value::Int(1),
            &source(MergePriority::Include, "first"),
            &Value::Int(2),
            &source(MergePriority::Include, "second"),
            "port",
            &mut provenance,
        )
        .expect("resilient");
        assert!(matches!(value, Value::Int(1)));
        let conflict = conflict.expect("conflict");
        assert_eq!(conflict.code(), "E046");
        let diagnostic = MergeError::Conflict(conflict).to_diagnostic();
        assert_eq!(diagnostic.code, "E046");
        assert_eq!(diagnostic.path.as_deref(), Some("port"));
        assert!(matches!(diagnostic.severity, crate::diag::Severity::Error));
    }

    #[test]
    fn blob_identity_ignores_payload_presence() {
        let without = Value::Blob {
            hash: [9u8; 32],
            size: 4,
            tainted: false,
            media_type: "text/plain".into(),
            payload: None,
        };
        let with = Value::Blob {
            hash: [9u8; 32],
            size: 4,
            tainted: false,
            media_type: "text/plain".into(),
            payload: Some(vec![1, 2, 3, 4]),
        };
        let mut provenance = MergeProvenance::new();
        unify(
            &without,
            &source(MergePriority::Local, "a"),
            &with,
            &source(MergePriority::Local, "b"),
            "blob",
            &mut provenance,
        )
        .expect("identity agrees regardless of the cache");
    }
}

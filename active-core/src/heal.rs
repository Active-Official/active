//! Self-healing engine (C-016).
//!
//! Phase 1 (syntactic repair) lives in the lexer (H001: unclosed strings)
//! and parser (H002: trailing commas, H003: missing commas, H004: bracket
//! recovery). This module hosts:
//! - Phase 2 (semantic key repair): similarity scoring and best-candidate
//!   selection over a set of legal keys, wired to schema-controlled
//!   tables in validation (H005 suggestions in strict mode, guarded
//!   auto-rename under resilient mode).
//! - Phase 3 (type/constraint healing): safe coercion functions, wired
//!   to validation mismatches (H007 under resilient mode) with
//!   sentinel recognition in strict messages.

use crate::diag::{thresholds, Diagnostic, Repair, RepairKind, Severity, Span};
use crate::ir::Module;
use crate::types::{ActiveType, Unit};

// ---------------------------------------------------------------------------
// Phase 2: similarity scoring for semantic key repair
// ---------------------------------------------------------------------------

/// Normalized string-similarity based on optimal string alignment (the
/// restricted Damerau-Levenshtein variant): 1.0 for identical strings,
/// 0.0 when every character differs. Case-insensitive, and a transposition
/// (the most common typo, `prot`/`port`) counts as a single edit.
pub fn normalized_levenshtein(a: &str, b: &str) -> f64 {
    let a = a.to_lowercase();
    let b = b.to_lowercase();
    let (al, bl) = (a.len(), b.len());
    if al == 0 && bl == 0 {
        return 1.0;
    }
    if al == 0 || bl == 0 {
        return 0.0;
    }
    let dist = damerau_levenshtein(&a, &b);
    1.0 - (dist as f64 / al.max(bl) as f64)
}

/// Optimal string alignment (the restricted Damerau-Levenshtein variant):
/// insertion, deletion, substitution, and adjacent transposition, each
/// costing one edit. Unrestricted Damerau-Levenshtein differs only for
/// substrings with repeated characters edited more than once; OSA is the
/// standard choice for typo detection.
/// The indexed loops mirror the classic DP table definition, where the
/// transposition rule needs the i-1/i-2 and j-1/j-2 neighbourhood.
#[allow(clippy::needless_range_loop)]
fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (al, bl) = (a.len(), b.len());
    if al == 0 {
        return bl;
    }
    if bl == 0 {
        return al;
    }
    let mut d = vec![vec![0usize; bl + 1]; al + 1];
    for i in 0..=al {
        d[i][0] = i;
    }
    for j in 0..=bl {
        d[0][j] = j;
    }
    for i in 1..=al {
        for j in 1..=bl {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            let mut best = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = best;
        }
    }
    d[al][bl]
}

/// Keyboard-layout similarity: how close two strings are on a QWERTY
/// layout. Returns 0.0–1.0; 1.0 means every character is on the same or an
/// adjacent key. Only meaningful for strings of equal length.
pub fn keyboard_similarity(a: &str, b: &str) -> f64 {
    if a.len() != b.len() {
        return 0.0;
    }
    let a = a.to_lowercase();
    let b = b.to_lowercase();
    let total = a.len() as f64;
    if total == 0.0 {
        return 1.0;
    }
    let matches = a
        .chars()
        .zip(b.chars())
        .filter(|&(ca, cb)| ca == cb || are_adjacent_keys(ca, cb))
        .count();
    matches as f64 / total
}

fn are_adjacent_keys(a: char, b: char) -> bool {
    let pos = |c: char| -> Option<(i32, i32)> {
        let layout = [
            "`1234567890-=",
            "\tqwertyuiop[]\\",
            " asdfghjkl;'",
            " zxcvbnm,./",
        ];
        for (row, line) in layout.iter().enumerate() {
            if let Some(col) = line.find(c) {
                return Some((row as i32, col as i32));
            }
        }
        None
    };
    match (pos(a), pos(b)) {
        (Some((r1, c1)), Some((r2, c2))) => (r1 - r2).abs().max((c1 - c2).abs()) <= 1,
        _ => false,
    }
}

/// Phonetic similarity via a simplified Soundex-style Metaphone. 1.0 when
/// the encodings match, 0.0 otherwise.
pub fn phonetic_similarity(a: &str, b: &str) -> f64 {
    if metaphone(a) == metaphone(b) {
        1.0
    } else {
        0.0
    }
}

/// Simplified Soundex-style encoding: keeps the first letter, collapses
/// consecutive letters with the same code, suppresses vowels after the
/// first letter, then pads/truncates to 4 characters. Crude but sufficient
/// for typo detection (`framwork` and `framework` encode alike).
fn metaphone(s: &str) -> String {
    let code = |c: char| -> Option<char> {
        match c.to_ascii_lowercase() {
            'b' | 'f' | 'p' | 'v' => Some('1'),
            'c' | 'g' | 'j' | 'k' | 'q' | 's' | 'x' | 'z' => Some('2'),
            'd' | 't' => Some('3'),
            'l' => Some('4'),
            'm' | 'n' => Some('5'),
            'r' => Some('6'),
            _ => None,
        }
    };
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i == 0 {
            out.push(c.to_ascii_lowercase());
        } else if let Some(d) = code(c) {
            if !out.ends_with(d) {
                out.push(d);
            }
        }
    }
    while out.len() < 4 {
        out.push('0');
    }
    out.truncate(4);
    out
}

/// Composite similarity score blending three independent signals with a
/// noisy-OR: edit distance (primary), keyboard adjacency, and phonetics.
/// Any single strong signal can push the score high, while weak signals
/// reinforce each other. 0.0–1.0.
pub fn key_similarity(typo: &str, candidate: &str) -> f64 {
    let lev = normalized_levenshtein(typo, candidate);
    let kb = keyboard_similarity(typo, candidate);
    let phon = phonetic_similarity(typo, candidate);
    1.0 - (1.0 - lev) * (1.0 - 0.5 * kb) * (1.0 - 0.5 * phon)
}

/// Choose the best matching key from a legal set. Only candidates with
/// similarity ≥ threshold are considered; on an exact tie the earliest
/// candidate in argument order wins.
pub fn best_key_match(typo: &str, legal: &[String], threshold: f64) -> Option<(String, f64)> {
    let mut best: Option<(String, f64)> = None;
    for k in legal {
        let score = key_similarity(typo, k);
        if score < threshold {
            continue;
        }
        let better = match &best {
            Some((_, best_score)) => score > *best_score,
            None => true,
        };
        if better {
            best = Some((k.clone(), score));
        }
    }
    best
}

/// Build an H005 (key typo suggestion) diagnostic.
///
/// The composite score ranks candidates, but it overstates certainty
/// (the three signals are correlated). The `safe` flag — i.e. permission
/// to auto-apply — therefore requires a near-identical edit distance,
/// not merely a high composite.
pub fn make_key_typo_diagnostic(
    typo: &str,
    suggestion: &str,
    confidence: f64,
    span: Option<Span>,
) -> Diagnostic {
    let safe = normalized_levenshtein(typo, suggestion) >= thresholds::AUTO_APPLY;
    Diagnostic {
        severity: if safe {
            Severity::Info
        } else {
            Severity::Warning
        },
        code: "H005".into(),
        message: format!("unknown key '{}', did you mean '{}'?", typo, suggestion),
        path: Some(typo.into()),
        span,
        repair: Some(Repair {
            original: typo.into(),
            suggested: suggestion.into(),
            kind: RepairKind::KeyTypo,
            safe,
            side_effects: vec![typo.to_string(), suggestion.to_string()],
            goal: None,
            steps: Vec::new(),
        }),
        confidence,
        // The caller stamps the file (same file as the typo); library
        // default is unset for single-file use.
        file: None,
    }
}

// ---------------------------------------------------------------------------
// Phase 3: type/constraint healing (safe coercion functions)
// ---------------------------------------------------------------------------

/// Try to coerce a string that looks like an integer. Rejects signs that
/// are more likely formatting than numbers.
pub fn try_coerce_int(s: &str) -> Option<i64> {
    let trimmed = s.trim();
    if trimmed.starts_with('$') || trimmed.starts_with('#') {
        return None;
    }
    trimmed.parse::<i64>().ok()
}

/// Try to parse a string as a duration: `5s`, `300ms`, `1.5h`. Returns
/// milliseconds on success.
pub fn try_coerce_duration(s: &str) -> Option<u64> {
    let trimmed = s.trim();
    let split = trimmed.find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-');
    match split {
        Some(pos) if pos > 0 => {
            let num: f64 = trimmed[..pos].parse().ok()?;
            let suffix = &trimmed[pos..];
            Unit::from_suffix(suffix)?.normalize_time_to_ms(num)
        }
        _ => None,
    }
}

/// Map a sentinel word to its conventional intent. Sentinels are not
/// values — they signal intent. The schema layer resolves the concrete
/// value (e.g. "unlimited" under Int(min=0) might become -1).
pub fn sentinel_meaning(word: &str) -> Option<&'static str> {
    match word.to_lowercase().as_str() {
        "unlimited" | "no_limit" | "max" => Some("limit"),
        "none" | "null" | "off" => Some("absent"),
        "inf" | "infinity" | "forever" => Some("infinite"),
        "any" | "all" | "wildcard" | "*" => Some("wildcard"),
        _ => None,
    }
}

/// Safe string → target-type coercion, applied only when a type error is
/// detected but the value is a string that could reasonably become the
/// expected type. Returns the coerced value or None.
pub fn coerce_string_to(value: &str, target: &ActiveType) -> Option<crate::ir::Value> {
    match target {
        ActiveType::Int => try_coerce_int(value).map(crate::ir::Value::Int),
        ActiveType::Float => value
            .trim()
            .parse::<f64>()
            .ok()
            .map(crate::ir::Value::Float),
        ActiveType::Bool => match value.to_lowercase().as_str() {
            "true" | "yes" | "1" | "on" => Some(crate::ir::Value::Bool(true)),
            "false" | "no" | "0" | "off" => Some(crate::ir::Value::Bool(false)),
            _ => None,
        },
        ActiveType::Duration => try_coerce_duration(value).map(crate::ir::Value::DurationMs),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Orchestrator
// ---------------------------------------------------------------------------

/// Run the self-healing passes over a module. Returns the number of repairs
/// recorded. Phase 1 repairs are already in `module.healing`; phases 2 and
/// 3 run schema-driven inside validation (H005/H007), which owns the
/// schema tree and the evaluated values they need.
pub fn heal(module: &mut Module) -> usize {
    module.healing.len()
}

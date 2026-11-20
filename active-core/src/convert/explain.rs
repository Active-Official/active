//! Deterministic failure explainer (CHECKLIST F.004, PROPOSAL §6.3).
//!
//! Maps diagnostic codes plus their spans to stable, value-free repair
//! guidance. No models, no heuristics, no randomness: the same code
//! always yields the same text, so agents can cache and quote it.
//! Guidance names the repair direction (fix the value, add the key,
//! loosen nothing silently); the `patch` surface turns the mechanical
//! subset into `@patch`/`@rename` directives.
//!
//! Coverage contract: [`explain_code`] pins canned guidance for the
//! codes enumerated in its match arms. Every other code (fatal
//! aborts such as X001–X006, unlisted E/T/W/S/H/L codes) falls back
//! to a generic pointer that preserves the code and span — the code
//! itself stays the machine-readable contract, never invented advice.
//! Reachability note: codes that abort collection (parse-fatal E*
//! via the `check` path, X* budget aborts, S001 cycle aborts) surface
//! with their code and span through the normal error report, so the
//! code is never lost even where no canned paragraph exists.

/// Stable one-paragraph repair guidance for a diagnostic code.
/// Unknown codes fall back to a generic pointer instead of inventing
/// advice — the code itself stays the machine-readable contract.
pub fn explain_code(code: &str) -> &'static str {
    match code {
        "E001" => "Expected a different token here: the document structure is incomplete or a delimiter is missing. Compare the span against the grammar (tables, brackets, commas) and close what is open.",
        "E002" => "Unexpected token at this position: something appears where the grammar allows nothing. Remove it or move it into the table it belongs to; do not quote it to silence the parser.",
        "E003" => "Malformed secret.env form: secret.env() takes a single string-literal name. Fix the call shape at the span; any other value form is rejected here.",
        "E004" => "Nesting cap exceeded: the parser allows 64 levels of expression nesting and 64 levels of schema nesting, fixed (no flag raises either). Split the expression with let bindings and flatten nested schemas.",
        "E006" => "Tab in leading whitespace: Active indents with spaces only. Replace the tab with spaces matching the surrounding level.",
        "E007" => "Inconsistent dedentation: the dedent does not match any open indent level. Align it with a level that is actually open.",
        "E011" => "Maximum block nesting depth (64) exceeded: flatten the document structure. The cap is fixed by the parser.",
        "E019" => "Patch failure. At parse time: a patch directive outside the document root, a missing paren or non-literal path, an interpolated path, an existing rename target, or an overlap with an immutable path. At apply time: the target changed under the patch or two patches collide. Fix the directive shape and rebase onto current paths.",
        "E020" => "Bad schema type expression at the span: unknown type or argument, malformed Array<T> / Enum(...) shape, non-boolean closed flag, or a schema block where schemas are not allowed (e.g. inside profiles). Fix the type expression.",
        "E023" => "Integer out of range: positive literals saturate at i64::MAX, and negating the minimum yields MIN+1. Use a smaller magnitude or a different unit.",
        "E024" => "Bad mode marker: active_mode must be the string \"data\" or \"config\". Fix the spelling; Data Mode is explicit opt-in, Config is the default when the marker is absent.",
        "E025" => "Bad capabilities declaration: active_capabilities must be an array of strings naming tools the host offers. Fix the shapes; unknown names are refused at handshake time, not here.",
        "E026" => "Include path is not portable: it uses a backslash separator or a Windows drive-letter absolute path. Use '/' separators and a path relative to the including file.",
        "E036" => "Interchange document is not self-contained: it reads `env.*`, includes a file, or declares profiles, all of which resolve against the RECEIVER's ambient state. Pin env values as literals, inline includes, and send the selected profile's values (LANG-F-007).",
        "E037" => "Signature expired: the sidecar's `expires` date has passed. Re-sign the file with a fresh expiry (or omit it deliberately); never disable the check.",
        "E038" => "Signing key revoked: the trusted-keys file lists this kid under `revoked`. Re-sign with a current key, or issue a trust file that does not revoke it.",
        "E039" => "Signature identity mismatch: the sidecar is bound to a different path (or to none). Sign the file for the exact include spelling the document uses (`active sign --as <path>`); a signed overlay cannot be replayed to another path (LANG-I-002).",
        "E035" => "Taint claim mismatch: `envelope.tainted` disagrees with the evaluated payload. A sender may not mislabel taint in either direction; make the claim match the payload (or remove it).",
        "E033" => "Envelope version refused: the Data Mode envelope's version is missing, is not a major.minor.patch string, or names a major this receiver does not implement. Send a version this receiver supports; never rewrite the receiver's gate to accept a foreign major.",
        "E034" => "Capability refused: the document declares a capability this receiver does not offer. Request only capabilities the receiver advertises, or widen the receiver's offered set deliberately (EnvelopeGate::with_tools).",
        "E032" => "Signature or trust failure: the sidecar is missing, mismatched, or untrusted. Verify against the trust file for the recorded key id; never bypass verification to make it pass.",

        "E028" => "Include budget: the include graph resolved more than 256 files or read more than 16 MiB. Flatten the graph (a diamond re-reads the same file) or move shared content into fewer files.",
        "E030" => "Internal panic isolation: a worker thread panicked and the run was aborted without partial output. This is a bug in Active, not in your document; report it with the input if it is shareable. E030 never describes language content.",
        "E029" => "Parse budget: the document crossed a parse-phase bound before evaluation - input bytes (4 MiB), arena nodes (1,000,000), or the parse wall budget (100 ms base plus 30 ms per KiB of input). Split the document, or raise the bound deliberately for generated input; the parse verdict is fail-closed, so no partial tree was evaluated.",
        "E027" => "Profile metadata rewrite: a profile body tried to set document metadata (active_mode / active_capabilities). The mode and capability handshake read those keys at parse time; declare them at the document root.",
        "T001" => "Unit or quantity violation at the span: an unsupported operator or dimension mismatch. Fix the unit expression; the code names the exact mismatch.",
        "T002" => "Type mismatch: the value's type does not satisfy the schema or operator at the span. Change the value to the expected type (or widen the schema deliberately), not the other way around.",
        "T003" => "Schema or format violation: the value breaks a constraint (format, pattern, range, membership). Fix the VALUE to a well-formed one, or correct the schema if the constraint itself is wrong — never delete the constraint to silence it. Unknown format names are schema bugs: use email|uri|uuid|semver.",
        "T004" => "Missing required key: add the named key. When the message names a trigger (`required by present`), the key is conditional on that trigger — set it or unset the trigger, but do not remove the requires rule.",
        "T005" => "Unexpected key in a closed table: remove the key, move it to the table it belongs to, or follow the did-you-mean suggestion (`@rename`). Opening the table trades a typo guard for silence.",

        "T008" => "Removed field still present: the schema scheduled it for removal at or before the document's schema version. Delete the field, or bump the schema version past the schedule only after removing it.",
        "T009" => "Root-schema check failed: a cross-field invariant named in the message is false, non-boolean, or not evaluable against the final document. Fix the values the rule constrains, or change the rule in the root schema's check block; a material or composite ==/!= operand is deliberately incomparable. Strict refuses; --resilient collects it beside the tree.",
        "T007" => "Reader version floor: the document declares a min_reader above the version this host declared. Upgrade the reader, or evaluate without declaring a version (the floors are advisory then).",
        "W004" => "Patch target warning: a @remove/@rename names a path that does not exist. Nothing was changed; check the path spelling.",
        "W005" => "Resilient default fill (informational): the engine inserted a schema default for a missing key. Harmless in resilient mode; strict mode requires the key written out.",
        "W013" => "Data Mode progressive warning: a budget is at 80% of its hard ceiling. Finish or shrink the document, or raise the limit explicitly and deliberately — hard ceilings bind Data Mode exactly as Config.",

        "W014" => "Diagnostic output truncated: the per-parse 1024-record budget (errors plus healing) was reached. The parse verdict is unchanged; fix the first errors to see the rest.",
        "L017" => "Sender-side self-containment: an interchange document (one with an `envelope` table) must not read `env.*`, include files, or depend on profiles - the receiver would resolve those against its own state. Pin the values before transmission (LANG-F-007).",
        "H005" => "Unknown key with a near-identical known key: apply the did-you-mean suggestion (`@rename`) when it matches intent.",

        "W019" => "Check truncation notice: more than 1,024 check rules failed, so the T009 list was capped. The document already refuses; fix the first rules and re-run. The parse verdict and the first 1,024 diagnostics are intact.",
        "W016" => "Ambiguous duration shape: a bare `{\"ms\": N}` object was imported as a table. Durations travel as `{\"$active.duration\": {\"ms\": N}}` (LANG-F-001/F-002); a sender that emits the bare shape is indistinguishable from one sending a table.",
        "W015" => "Trailing-token warning: a token follows a complete value on the same line. Active statements are newline-separated; the documented quirks (`x = 5 5`, `d = 1.`) are the only silent shapes. Put the next statement on its own line.",
        "H006" => "Repair stuck: the same diagnostics returned after another repair step, so the loop stopped instead of cycling. Inspect the plan's applied steps; the remaining diagnostics need a repair kind the suggester does not emit.",
        "H008" => "Unrepairable terminal state: remaining diagnostics have no safe repair mapping (or evaluation fails fatally). The agent loop must stop here; report the codes, do not retry blindly.",
        "S001" => "Dependency cycle: the listed keys depend on each other and no evaluation order exists. Break the cycle by removing one dependency edge.",
        "B001" => "Malformed blob() call: blob() takes a string media type and string (or secret) data — blob(\"text/plain\", \"...\"). Fix the argument shapes at the span.",
        "B002" => "Blob payload exceeds the schema max_size: shrink the payload or raise max_size deliberately, never strip the bound to silence it.",
        "B003" => "Blob materialisation refused: the blob is structurally tainted. Tainted payloads are never emitted — redesign so the consumer needs only the identity.",
        "B004" => "Blob media type mismatches the schema media_type: the bytes are not what the schema promises. Fix the data or the declared type.",
        "B005" => "Blob payload unavailable: the identity was re-imported without retained bytes. Re-evaluate from the source document with materialisation enabled.",
        "C040" => "Compatibility: a required field was added — old documents fail the new schema. Add a default, make it Null-typed, or accept the break explicitly.",
        "C041" => "Compatibility: a field was removed — old readers and writers lose data. Restore it, or confirm the removal was scheduled (removed_in at or before the new version).",
        "C042" => "Compatibility: a field type changed (or a blob constraint tightened) — existing documents and payloads may fail. Migrate the data or version the schema.",
        "C043" => "Compatibility: the schema version regressed — the new version is older than the old one. Versions only move forward.",
        "C044" => "Compatibility: min_reader or min_writer was raised above the old version — old readers or writers are dropped. Lower the floor or accept the break explicitly.",
        "C045" => "Compatibility: a root-schema check rule was added, removed, or changed - documents that passed may now fail (or the reverse). Review the rule and version the schema deliberately.",
        "E040" => "Breaking schema changes detected: the compatibility report lists error-severity findings that must be resolved or explicitly accepted.",
        "E041" => "Include content pin mismatch: the include's bytes are not the ones the document or lockfile pinned. Review the bytes, then update the refusing pin: an inline `sha256:` in the importing document, or re-run `active lock` for a lockfile pin. Never edit an include merely to make its pin match.",
        "E042" => "Envelope v2 receipt missing or malformed: a v2 result must ship an audit receipt (outcome, failure class, diagnostics, result) even when the attempt failed. Add a `receipt` table with `outcome = \"clean\"` or `\"failed\"` and a `sha256:` result.",
        "E043" => "Envelope v2 sender or result malformed: `sender` must be a table with a non-empty `id`, and key/signature are given together (signature 128 hex). `result` must be `sha256:<64 hex>`. Signature validity is a receiver-policy check.",
        "E044" => "Envelope result does not match the bound value: recompute `envelope::bound_content_hash` over the redacted payload plus schema version, profile, resolved budget, mode, and capability, and compare `result` and `receipt.result`. The pre-gate cannot see profile/patch rewrites; this is the post-evaluation check.",
        "E045" => "Sender signature cannot be verified: pass `--envelope-trust <trust.toml>` with the sender key id listed, confirm the key is not revoked, and re-check that the signature covers the bound result plus the canonical receipt digest. An id-only sender is an assertion, not authentication.",
        "E046" => "Merge conflict: two includes of the same priority disagree at the named path. Make one value local, align the values, or move the override into a higher-priority layer; resilient evaluation keeps the first source in source order. Repeated headers still keep last-wins and a selected profile keeps its current override warning (W003) until phases 3 and 4; two bodies for one profile name are already E018.",
        "W017" => "Unpinned include under a legacy lockfile: the lock predates include pins and admits matching content by hash. Re-run `active lock` to record spellings and resolved paths, or add an inline `sha256:` pin to the include.",
        "W018" => "Include store write failed: the cache directory is missing, read-only, or full. Evaluation used the bytes already read, so the tree is unchanged. Fix the store path or permissions, or drop `--store` when the cache is not wanted.",
        _ => "No canned guidance for this code: the code and span are the contract — read the message, fix the span it names, and re-run check.",
    }
}

/// One stable guidance line per diagnostic: `CODE path span: guidance`.
/// Message text never enters the output (value-free by construction).
pub fn explain_all(items: &[(String, Option<String>, Option<String>)]) -> Vec<String> {
    explain_structured(items)
        .iter()
        .map(Explanation::line)
        .collect()
}

/// A single machine-readable explanation. `span` uses the same
/// `line:col-end_line:end_col` text form as [`explain_all`] so the two
/// renderings stay byte-comparable field by field.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Explanation {
    pub code: String,
    pub path: Option<String>,
    pub span: Option<String>,
    pub guidance: &'static str,
}

impl Explanation {
    /// The canonical text rendering (identical to [`explain_all`]).
    pub fn line(&self) -> String {
        let where_ = match (&self.path, &self.span) {
            (Some(p), Some(s)) => format!("{p} {s}"),
            (Some(p), None) => p.clone(),
            (None, Some(s)) => s.clone(),
            (None, None) => "document".to_string(),
        };
        format!("{} {where_}: {}", self.code, self.guidance)
    }
}

/// Structured explanations in diagnostic order. Same content as
/// [`explain_all`], shaped for agents to parse instead of scrape.
pub fn explain_structured(items: &[(String, Option<String>, Option<String>)]) -> Vec<Explanation> {
    items
        .iter()
        .map(|(code, path, span)| Explanation {
            guidance: explain_code(code),
            code: code.clone(),
            path: path.clone(),
            span: span.clone(),
        })
        .collect()
}

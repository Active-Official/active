# Security hardening record (review rounds 12-75)

This record summarizes the independent review loop that hardened the
Master Program tree before release. It complements
`spec/secrets.md`, `spec/blob.md`, and the completion record.

## Method

- Reviewer: DeepSeek V4.1 Flash through the Copilot CLI BYOK route
  (CommandCode), `--reasoning-effort max`. The Copilot subscription
  quota was exhausted for the review window; the routing is documented
  in the review skill and never carries credentials into the repo.
- Loop: after every fix round the reviewer re-ran, confirmed the previous
  findings, and searched for new ones. Each real finding was reproduced,
  fixed at the cause, and pinned with a regression test that drives the
  shipped path (CLI command, exporter, inspector, trace, FFI/WASM entry).
- Gates after every round: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, plus the wasm target check.
- Final verdict and log: `conformance/evidence/review-final.log`.

## Finding classes and resolutions

| Class | Examples | Resolution |
|---|---|---|
| Secret leakage through exports | env-backed copies, profile-selected values, profile-authored literals, `let`-bound computed secrets, blob payload bytes, provenance env values | one redactor seeded from authored literals, the evaluator's material node values, profile-authored literals, and material env values; every exporter/inspector/trace/lock/audit absorbs it before rendering |
| Secret leakage through tools | inspect wrappers, debug steps, LSP hover, graph/viz taint, audit/replay hashes, reduction bundles | identity-only step and inspect rendering, path-aware hover, seeded taint queries, canonical hashing, evaluator-seeded duplicate/comment redaction |
| Availability | quadratic redaction, work-budget over-redaction, per-alias payload charging, trusted-budget passes | Aho-Corasick batches with match cursors, per-batch retirement, a taint probe that skips alias copy accounting, caller-honoring budgets |
| Fidelity | longest-at-equal-start semantics, marker opacity, prefix preservation, UTF-8 | differential tests against a naive leftmost-longest reference, marker-split scanning, cursor reuse |
| Contract | `tainted` flags, audit/replay/lock agreement, error codes | flags derived from the same decision as the value; one canonicaliser for audit, lock, and replay |
| Bounds and determinism | fail-open matcher builds, profile truncation, zero/unlimited limits, i64 document limits | linear fallback, fail-closed marking, per-field limit validation, deterministic fold/batch state |

## Late rounds (60-71)

The tail of the loop moved from "a value reaches an output" to "the model
is consistent and bounded on every transform":

- Post-transform profile seeding: the node at each material path is
  resolved after every profile and patch, with cycle-safe memoization and
  deterministic root order; a computed selector seeds its maintained path
  records so a sibling patch cannot erase a surviving value.
- Visibility: export visibility is filtered by the evaluated export paths
  (not binding names or failed Sets); a computed selector drops every path
  a possible profile could override.
- Exact vs substring: whole-value matching at any length, substring
  spreading at four bytes and up, any-length authored-prose redaction.
  The exact set reaches export, inspect, T006, constraint sources, docgen,
  LSP, blob identity/payload, JSON redaction, and debug duplicates.
- Diagnostics: B004/T003/T004, T006 sources and actuals, L005 reasons, and
  parse-time E0xx messages are value-free or redacted with the engine
  environment; T004 keeps a resolvable `requires` target readable.
- Generators: enum members, defaults, patterns, formats, and media types
  are withheld from executable slots when material rather than replaced
  with a marker, so generated artifacts stay valid.
- Planning and repair: one analyzed profile per pass (linear), built from
  the engine's profile override and environment; a fill that would copy
  material is withheld, and directives keep their structural paths.
- Debugger: malformed input is covered by literal-island runs,
  split-interpolation elision, malformed-string slices, bare-token runs,
  inter-token gaps, and a fail-closed sweep, while canonical env names and
  structural literals stay reproducible.
- Availability: iterative toposort replaced a recursive cycle check,
  interpolation nesting charges the depth budget, and long alias chains
  fail closed at fixed work/byte/depth bounds instead of overflowing the
  stack.

The local working record with the full round-by-round reasoning is
`master-program/HARDENING-REVIEWS-12-71.md` (gitignored, never pushed).

## Invariants held

- No raw secret or `payload_b64` reaches an output surface in any tested
  path; reduction also redacts duplicate values, comments, and
  base64/hex encodings of a known secret. The only payload emitter is
  the explicit `eval --materialise-blobs` path, which refuses a
  secret-bearing payload with B003.
- Redaction is linear per pattern batch and never marks a benign value
  wholesale.
- A bounds trip fails closed (marker), never open.
- Repeated exports are byte-identical; audit, lock, and replay hash the
  same canonical tree.

## Evidence

- `conformance/evidence/review-final.log` - final verification round
  verdict: `CLEAN`, over the complete goal diff.
- `conformance/evidence/gates.log` - Rust gates on the final tree.
- `active-core/tests/secrets.rs`, `inspect.rs`, `debug.rs`, `blob.rs`,
  `graph_queries.rs`, `viz.rs`; `active-cli/tests/cli.rs`;
  `active-lsp/tests/tooling.rs` - the regression tests added by the loop.

## Post-loop hardening (2026-10-25, rounds 73-74)

After the round-72 clean verdict, an independent audit re-read the two
program checklists against the tree. It found checklist boxes whose
delivery was narrower than the step text, plus documentation that had
drifted. Each gap was closed in code and pinned with a test; the final
full-diff review then ran over the larger tree (round 73) and a
verification pass confirmed the fixes (round 74).

### Linter: the six missing rule categories

The step text names six categories (correctness, schema/evolution,
agent ergonomics, style, performance, mode). Ten rules existed; four
categories were unrepresented and project configuration was absent.

- **L011 `requires-discipline`** — a `requires` target that is empty,
  self-referential, or does not resolve to a schema path is a dead
  conditional that reads as a working contract. It reuses
  `schema_path_exists`, so the linter and the schema exporters agree on
  what a real target is.
- **L012 `agent-annotations`** — an enforced `@constraint` with no
  `@intent` prose. Agents need the reason to repair a violation; a bare
  predicate is the ergonomics gap the category names.
- **L013 `ambiguous-repair`** — sibling keys that normalize to the same
  case/punctuation-insensitive identity. A rename repair cannot choose
  between them, so the ambiguity is reported before an agent spends a
  repair loop on it.
- **L014 `canonical-order`** — `depends_on` after ordinary keys. The
  span comparison uses the parser's recorded lines, so it sees the
  declaration even though `depends_on` is extracted from the value tree.
- **L015 `mixed-syntax`** — one document using both brace and
  indentation table blocks. The parser already counted both styles per
  file; the count is now exposed on `Module::mixed_syntax`.
- **L016 `include-hygiene`** — absolute paths, parent escapes,
  non-`.active` targets, and duplicate declarations. This is defense in
  depth: the parser refuses absolute/escape paths by default (E015), and
  the rule reports them when a caller opts in.

`.active-lint.toml` adds project configuration: a `disable` array and a
`[severity]` table (`off|error|warning|info|hint`). Discovery walks from
each input file upward; `--config` pins a file. Unknown keys, unknown
rule names, and malformed discovered files are errors, because a typo
that silently disables a rule is worse than a build failure. Severity
overrides apply after rule evaluation and flow into text, JSON, and
SARIF; `off` drops the finding.

Why this is better: the linter now covers every category the design
names, the rules are independently disableable per project, and the
report severity is a project decision instead of a constant. The
`clean_corpus_and_examples_raise_nothing` invariant still holds; the one
deliberate dual-syntax acceptance fixture is the single documented
exemption.

### Debugger: budget breakpoints and reproduction inputs

- `budget:<percent>` matches the first step whose cumulative
  evaluated-node count reaches that percentage of the configured
  `max_nodes`; `budget:nodes:<n>` matches the n-th evaluated node. The
  CLI passes the resolved budget, so a breakpoint and the limit that
  stopped evaluation can never disagree. A small document under a large
  budget correctly never trips a percentage breakpoint.
- The `--minimize` report now exports the reproduction inputs: the
  environment variable **names** the document reads (sorted, deduped,
  values never read) and the effective budget with the wall clock shown
  as 0 when the reducer disabled it for determinism. A reduced case can
  now be replayed under exactly the limits that produced it.
- A `budget:nodes:0` parse path used `then_some(n - 1)`, which
  evaluated the subtraction eagerly and underflowed; `then` fixed it and
  the test pins the boundary.

### Formatter: a real round-trip bug found by the new sweep

A `formatter` fuzz target was added (no panic, output re-parses,
idempotent) and the same contract is exercised by a deterministic
mutation sweep over the positive corpus, so the claim is testable where
libFuzzer cannot run. The sweep found a real defect: `secret.env("NAME
WITH SPACE")` formatted to the bare `secret(env.NAME WITH SPACE)` form,
which re-parses as a shorter name. Names that are not bare identifiers
now keep the string form (`is_bare_identifier`), so the value survives a
round trip byte-for-byte in both styles.

CI also gained an `active fmt --check` job over canonical brace and
indent corpora (`tests/fmt/`), which makes the formatter's CI mode a
real gate rather than a documented feature. The showcase examples stay
authored (aligned columns, section blank lines) and outside the gate;
that choice is recorded in `spec/formatter.md`.

### Repair browser: filters

`--filter-code`, `--filter-severity`, and `--filter-path` narrow the
browser's diagnostic list; they intersect and validate severity names.
The plan is never filtered, because `--apply-step` and `--cherry-pick`
address plan indexes and hiding steps would change apply semantics.

### Visualizer: compatibility and migration view

`viz schema` now carries `compatibility`: the declared version,
`min_reader`, `min_writer`, the deprecated fields with their authored
migration hints, and the removal schedule with generated hints. It is
derived from the schema tree (paths and prose only), sorted for
determinism, and still passes through the CLI's material profile, so
prose equal to a host value redacts like every other prose surface.

### Language server: the three missing bridges

- `active/capability` serves the embedded descriptor and fails closed on
  `requireMajor` mismatch, so a client can dispose tools before asking
  for anything else.
- `active/explain` returns the structured explanations for every live
  diagnostic; hover also appends the canned `_fix_:` guidance to lint
  findings it shows.
- `active/debug/steps` returns the redacted step trace with the same
  breakpoint selectors as the CLI, evaluated under the same resilient
  policy as diagnostics.

All three run the shipped engines; the server adds no second
implementation. `spec/lsp.md` now documents every custom method in one
table.

### Capability descriptor: 20 tools

The descriptor advertised 13 tools, so `inspect --path`, `debug`, `viz`,
`budget`, `mode`, `doc`, and `schema inspect` were undiscoverable. The
descriptor now lists 20, the data/agent/trusted profiles carry the
matching subsets, and the CLI mapping test proves every advertised tool
resolves to a real subcommand. The Python authority test and the Rust
`KNOWN_CAPABILITIES` pin moved with it, so the three renderings cannot
drift.

### Interchange: three envelopes and an executable short-circuit

- `examples/agent-interchange/` now ships a tool result, a plan, and a
  world model; the test iterates the directory and asserts Data Mode,
  the required fields, the handshake for each named profile, and no raw
  secret in any export.
- `spec/agent-interchange-profile.md` §7 specifies the "I already
  possess hash H" exchange, and `agent-kit/interchange/` ships
  `normalize_identity`, `short_circuits`, and `missing_identities` with
  a vector pinned to a real blake3 identity from the world-model
  example. Taint never changes identity; malformed identities never
  match; a record is a leaf, so a probe cannot force payload
  materialisation.

### Program records and CI

- `spec/repair.md` now documents the repair engine (goals, plan,
  apply, terminal states, safety); the completion record's citation is
  true instead of dangling.
- `README.md` gains a Config/Data modes section and an evidence index;
  `docs-site/src/reference/modes.md` is a full modes reference.
- The playground transcripts are now a checked-in artifact
  (`playground/loop/transcript.txt`), captured from the shipped binary.
- CI uploads the conformance report with 30-day retention and fuzz
  artifacts with 14, which is the retention configuration B.006 asked
  for.
- The completion record was corrected where it named the wrong code
  (stuck H006 vs unrepairable H008), the wrong test crate for audit
  re-validation, the wrong output-schema count, and an unqualified
  module path.

## Round 73 - full-diff review findings and fixes

The final full-diff review over `ef98a2d..HEAD` (plus the working tree)
returned 12 findings. All twelve were reproduced, fixed at the cause,
and pinned with a regression test that drives the shipped path.

### CRITICAL

1. **Schema inspect under `--profile` leaked env-resolved material.**
   `schema_inspect_with_profile` analyzed the override, but the second
   redaction pass built the material profile from the document selector
   alone, so an env value selected only by the override never entered
   the material set. Fix: `material_profile_with(module, override)` uses
   `analyze_with_profile` and absorbs the environment values that the
   override makes material. Regression:
   `inspect_schema_redacts_material_under_a_profile_override`.
2. **Compatibility findings printed env-equal enum members.**
   `compare_schemas` redacted with the document selector; a value
   copied from an env read into an enum member or media type was not
   material there. Fix: the CLI sweeps every finding message again with
   both documents' material profiles (old and new). Regression:
   `compatibility_redacts_an_environment_equal_enum_member`.

### HIGH

3. **Sub-four-byte structural secrets were spread as substrings.**
   The structural seed loop inserted every locator literal at any
   length, and the runtime seed path did the same, so `secret("a")`
   turned "banana" into marker confetti. Fix: the 4-byte floor now
   applies at every substring insertion (structural seeds, path seeds,
   runtime seeds, absorbed env, exact-copy path); whole-value matching
   stays unlimited via `exact_literals`. Document data uses the split
   `redactor_and_exact_with_runtime_secrets` (exact set + gated
   substring set); authored prose keeps the full sweep. Regression:
   `a_short_structural_secret_never_rewrites_unrelated_text`.

### MEDIUM

4. **A blob whose payload copied a secret exported `tainted: false`.**
   Identity-only evaluation cannot inspect payload bytes, so taint is
   now decided at construction: the payload text is checked against the
   authored material set and the closed env snapshot, and a hit marks
   the blob tainted with no retained payload. `eval`, `convert`, `doc`,
   `debug`, `inspect`, `lock`, and `audit` now agree. Regression:
   `a_blob_whose_payload_copies_a_secret_is_tainted`.
5. **`debug --minimize` skipped short secrets in comments.**
   Comment redaction used the 4-byte `redact_text_min`; the spec says
   comments use substring matching throughout. Comments now use
   `redact_text` (the full set), while document data keeps the floor.
   Regression: `minimize_redacts_a_short_secret_in_a_comment`.
6. **Docgen was quadratic per prose field.** `redact_every_occurrence`
   rebuilt a full redactor (and its automata) on every call. The
   redactor already holds every value, so the method now delegates to
   `redact_text`; docgen on the 310 KB/800-secret probe drops from
   ~22 s to linear time.
7. **Empty blob media types were accepted.** `blob("", "x")` produced a
   valid identity with an empty media type, contradicting spec §1.
   Construction now fails with B001 for literal and dynamic empties.
   Regressions: `an_empty_blob_media_type_is_b001`,
   `a_dynamic_empty_blob_media_type_is_b001`.
8. **`inspect --path` disagreed on `required`, and `\$` defaults were
   treated as dynamic.** A single static-default resolver
   (`static_default`) now backs JSON Schema/OpenAPI, `schema inspect`,
   `inspect --path`, docgen, and the fill planner, and it resolves an
   all-literal interpolation. Regressions:
   `an_all_literal_interpolation_default_is_static_everywhere`,
   `a_dynamic_default_counts_as_required_on_every_surface`,
   `a_static_interpolation_default_is_fillable`.
9. **An included `active_profile` was silently ignored.** The include
   splice skipped it together with mode/capabilities, regressing the
   grammar's "included files provide defaults" rule. Mode and
   capabilities stay local; the selector is an ordinary root value
   again. Regression: `an_included_profile_selector_is_a_root_default`,
   and `included_mode_and_capabilities_stay_local` pins the remaining
   local metadata.

### LOW

10. **Nested schema evolution metadata was invisible to L006/L010.**
    Both rules now walk every schema table (`schema_nodes`), so a nested
    `min_reader`/`version`/`removed_in` behaves like the root.
    Regression: `evolution_rules_see_nested_schema_tables`.
11. **The Rust generator emitted a marker enum variant.** A material
    enum member now degrades the field to `pub type X = String;`
    (matching TS/Python/Go) instead of a `***SECRET***` rename that
    accepted the marker. Regression:
    `a_material_enum_member_degrades_to_string_on_every_target`.
12. **No conformance vectors for the new surfaces.** Added
    `0017-blob-identity`, `0018-blob-tainted`, `0019-mode-data`,
    `0020-mode-invalid` (E024), and `0021-capability-shape` (E025) with
    committed expectations. The differential gate now runs 19 cases
    with identical codes, spans, and values across the C ABI, Python,
    and JS embeddings.

Round 74 verified 11 of the 12 as fixed and flagged F6 for a missing
discriminating test. Round 75 verified the added docgen bound: the
committed test passes in 0.25 s, and restoring the per-call redactor makes
it fail at 3.80 s. Final verdict: all 12 fixed.

The round also rebuilt the release C ABI and the debug WASM module so
the differential gate exercises the fixed core rather than a stale
artifact.

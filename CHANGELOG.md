# Changelog

All notable changes to the Active language, toolchain, and
specification. (Entries below the rename keep their original
SOLID-era names: that history is accurate as written.) Versions
follow SemVer with a unified workspace version (spec/packaging.md §1);
the `active-infinity` name is not part of this workspace (VERSIONING.md
§4 - the placeholder member was retired, and the name belongs to the
standalone Infinity codec).

## [Unreleased]

### L-P38: property tests as the spec

Adds generated property tests (`active-core/tests/properties.rs`) for
determinism, formatter idempotency, formatting semantic preservation,
the JSON wire fixed point, budget monotonicity, and secret
non-leakage. The wire property found and this change fixes three
spelling defects: quantities now import with a float spelling
(`5.0MiB`), zero renders signless for Float/Quantity (`-0.0` -> `0.0`,
G13), and an `ms`-suffixed duration literal accepts the full u64 range
while plain integers stay i64 with E023. `spec/converters.md` now
states that blob identities are export-only (P38-F1).

### L-P37: reference interpreter and differential testing

A second, independent implementation of Active's core semantics in
Python 3.12 (`tools/reference-interpreter/`, a test oracle outside the
workspace and never imported by a shipped crate) now agrees with the
shipped engine on **all 69 conformance cases and all 300 Monte Carlo
documents, with zero divergences**. The runner compares redacted JSON
values, ordered diagnostic records (code, severity, path, and span),
duplicate counts, and `content_hash`; a leak scan rejects any known raw
fixture secret and every runtime seed; two runs produce byte-identical
reports. A new required CI job (`reference-differential`) dumps the
deterministic MC corpus twice from the existing sweep
(`ACTIVE_MC_DUMP`), requires the dumps to match, and runs the
differential with the 69/300 count gate.

The implementation carries Bud's 21-gap adjudication into the spec:
normative recovery (H001-H004/W015, G05/G04), E002 for an unknown unit
run and integer-only underscores (G06), the shipped float spelling
(`1e+21`, `-0.0` -> `0.0`, G13), the canonical hash tuple (G14), the
complete operand matrix including same-dimension equality and the
quantity-scalar asymmetry (G18), the redaction passes (G19), the
document-salt layout (G20), the W013 order (G21), manifest-derived
conformance counts (G22), the three-key root closed-table exemption
(G23), the pattern dialect (G24), the structured diagnostic record/order/
spans (G01-G03), env snapshot semantics (G08), profile replacement (G10),
include post-order (G11), and the schema-parse table (G17). The G20 note
is corrected by evidence: the shipped salt domain hashes
`active-doc-salt/v1` with no trailing `0x00`, and the tainted 0018
identity reproduces byte-exactly.

Two code fixes land with tests and negative controls: cycle detection now
precedes the static shape budgets (G16; a cyclic over-budget document
reports S001, not X001), and `active_profile` joins the root
closed-table exemption (G23; a closed-root document that selects a
profile no longer fails T005). Composite equality stays shipped-false
(P37-N1, later candidate); secret-derived scalars redact by path
(P37-N2, now normative in `spec/secrets.md`).

The oracle also mirrors the envelope v2 receipt/sender/result shape and
bound-result check, final-tree schema checks and `@constraint` T006,
all-profile evaluation, included immutables/metadata/patch ordering,
UTF-8 byte secret thresholds, and the schema/profile/include document
salt projection; 62 oracle unit tests and 69/69 corpus plus 300/300 MC
records/spans/hashes are green.

### L-T05: `active serve` (stdio document transport)

A long-lived stdio server for Active documents: `u32` little-endian
length-prefixed JSON requests (`{id, source, name?, env?}`) and one L-T06
envelope v2 per response (`{id, envelope}`). The 16 MiB frame cap is
enforced from the length prefix before any allocation (an over-cap frame
gets one clean `E029` error frame and the server stops); a document over
the parse cap is refused before parsing. The server is hermetic: no
include reads (an `include` is E015) and no ambient environment (only the
request's `env`), carries no state between requests, and binds the same
`result` hash as `active eval --envelope`. Spec `serve.md`, vectors
`conformance/serve/`, docs `tooling.md`.

### L-P62: propose / gate loop with the verifier as the product

`active gate` verifies one candidate with the same per-file checks
`active test` runs (strict parse, healing refusal, canonical format,
strict evaluation and validation, project lint) and accepts or rejects
it; `--receipt` writes an L-T06 envelope v2 binding the redacted accepted
value. `active propose` runs the bounded verify → repair → re-verify loop
over a caller-supplied candidate or a `--generator` command (the request
goes to its stdin, the candidate comes back on stdout); the loop defaults
to two rounds, caps at five, and fails closed with H008 naming the
remaining checks. An output file is written only for an accepted
candidate. The `active verify` name stays the signature command; the
candidate verifier is `gate`. Spec `propose-verify.md`, vectors
`conformance/lp62/`, docs `tooling.md`.

### L-P53: repair steps carry proof sketches

Every applied step of `active repair` now carries a `proof` sketch checked
by the loop's next evaluation: the `CODE@path` diagnostics it removed, the
ones it introduced (none for a verified step), and every evaluated path
outside its own blast radius whose value changed. Both trees are redacted
before comparison, so material never enters it. The comparison is bounded
(65,536 visited values, 16 reported paths) and fails rather than
truncating. `--format text` prints a `proof:` line under each applied
step; the JSON plan and the browser model (which embeds the plan) carry
the object, and `docs/schemas/repair-plan.json` documents the shape.
Spec: `spec/repair-ui.md` section 3. Internally, `Engine::evaluate_tree`
runs the walk with validation disabled so the planner can compare trees
while the validation diagnostics are its input.

### L-P09: unification as the one merge semantics (2.0.0 language change)

Every source merge is one operation with priorities Default < Include <
Local < Profile. Tables merge recursively; equal values agree; a scalar,
array, or shape disagreement between same-priority sources is E046. The
first source in source order remains in resilient output; strict and
resilient both report the error. Provenance is a bounded side map
(65,536 paths, 8 MiB of path/source text, 16 losers per path) exposed by
`active inspect --path` (value report v2: `merge` plus truncation flags)
and by `Engine::evaluate_with_merges`; records never contain values.

Migration fallout: includes are equal-priority peers, so a document that
relied on the last include winning now reports E046 when the includes
disagree (a local key still beats every include). Repeated table headers
and brace/indent bodies merge instead of discarding the earlier body;
W011 no longer fires for table bodies (duplicate scalar keys keep it).
Schemas in repeated bodies follow the same Local/Include direction and
keep the first `closed` flag. Profiles merge at Profile priority, a
profile type change is not a warning (W003 is retired and reserved), a
material `active_profile` selector refuses with E027, and a profile's own
`active_profile` is allowed but never reselects or replaces the exported
selector. A table body cannot extend an alias (E046). Dotted headers cap
at 256 segments (E011). Conformance vectors 0043-0048.

### L-P19: root-schema `check` blocks (cross-field invariants)

A root schema may declare `check { name = predicate }` (or `check:` with an
indented body). Predicates are authored expressions evaluated against the
FINAL tree after schema validation, `@constraint`, profiles, patches, and
resilient fills, in declaration order. Identifiers are root keys; dotted
references and interpolations resolve through their authored paths.
Material and non-scalar `==`/`!=` operands are incomparable (T009), so a
check can never become an equality oracle. A false, non-boolean, or
unresolvable predicate is T009 naming the rule (redacted); strict refuses,
resilient collects. Checks are root-file/root-schema only: nested data
schemas and included files reject them (E020), and a duplicate schema's
discarded checks never run. The formatter emits the authored predicate
text in both styles, so dotted references survive. `active inspect`
lists checks (redacted) and `active compatibility` reports C045 for
added/removed/changed rules; code generators document that cross-field
checks are not representable. Spec `validation.md` section 11, grammar
2.10, diagnostics T009/C045; conformance vectors 0041/0042.

### L-P16: plan/apply split (`active plan`, `active apply`)

`active plan old new` shows what replacing the baseline with the
candidate does: the strict semantic diff, affected dependents, a risk
class (`breaking`/`behavior-change`/`review-required`), the steps an
approver accepts, and a per-user keyed plan hash (a MAC) over the
canonical plan. Paths are relative to `--root` (default: the inputs'
common ancestor); nothing is written unless `--output` names a plan
file, and `--fail-on-breaking` turns a breaking risk into a nonzero
exit. `active apply <plan> --approve <mac>` requires the plan's keyed
hash, confines both paths under `--root`, re-runs the strict review
(context, document and include hashes, diff, risk) on the exact files,
replaces the baseline atomically while preserving its mode (ACLs are not copied), and
writes a deterministic JSON receipt with the plan hash, root, both
hashes, the optional operator, and the receipt's own keyed hash. An
existing receipt is refused without `--force`; `apply` is offered by the
`trusted` sandbox profile only. `docs/schemas/plan.json` and
`plan-receipt.json` pin the shapes.

### L-P52: semantic diff (`active diff`)

`active diff <old> <new>` compares evaluated documents rather than text:
added, removed, and changed value paths with both type names and redacted
scalar echoes, schema compatibility findings (C040-C044), the dependents
of each changed path, and whether every compared value is equal.
Material is never compared: a secret or blob always reports
changed with a `***SECRET***` echo, so a diff cannot confirm a guess
(the two documents' material sets are unioned, so a value secret on one
side redacts on both), and every path (value, schema, impact) is redacted
segment by segment. The
impact section is bounded (64 keys / 4096 paths, `impact_truncated`).
`--format json` is pinned by `docs/schemas/diff.json`; text is the human
rendering. Both documents parse and evaluate strictly: an error or
warning refuses with its stable codes and a nonzero exit; schema findings
do not gate.

### L-P29: content-keyed evaluation cache

`active eval --cache <dir>` serves a repeat run from a stored entry when
the source and include hashes, mode, profile, resolved budget, input path,
environment values, and output variant are unchanged. Entry names are a
keyed BLAKE3 MAC using a per-user key file outside the cache directory
(`$ACTIVE_CACHE_KEY_FILE` or the platform config directory), so a
directory listing cannot confirm a guessed environment value or forge an
entry. Entries hold the JSON/YAML/TOML renderings, diagnostics, and the
L-T06 v2 receipt; they are read through a bounded regular-file reader and
rechecked after open. Writes are atomic; the root is 0700 and entries
0600 on Unix, with shared roots/entries refused. A material profile or
schema selector writes no entry, and an output beyond 32 MiB is printed
but not cached. Run observers and admission flags (`--audit-trail`,
`--envelope`, `--trace`, `--store`, `--trust-keys`, `--lockfile`,
`--envelope-trust`) bypass the cache with a warning. Spec
`reproducibility.md` section 4d.

### L-T06: envelope v2 with receipts and sender identity

The tool-result envelope gains major 2 (majors 1 and 2 are accepted;
unknown majors stay E033). A v2 envelope carries:

- `receipt` (required): the audit record for the attempt - `outcome`
  (`clean`/`failed`), `failure_class` (required when failed), diagnostics,
  the bound result, schema version, profile, mode, resolved budget, and
  include resolutions. A missing or malformed receipt is E042, so a
  parse/budget failure is exchangeable instead of an empty envelope.
- `sender` (optional): `{ id, key?, signature? }`. Key and signature are
  given together; the 128-hex Ed25519 signature covers the length-prefixed
  sender id, key id, envelope version, bound result, and canonical receipt
  digest (`envelope::sign_sender` / `verify_sender`). A malformed
  sender/result is E043; signature validity is a receiver-policy check.
- `result` binding: v2 hashes the key-sorted canonical tuple of payload +
  schema version + profile + resolved budget limits + mode + capability
  (`envelope::bound_content_hash`), closing the v1 gap where the hash
  bound only the value tree (LANG-F-009). A clean v2 result is required,
  `result` and `receipt.result` must be equal, and the post-evaluation
  check (`check_evaluated`, E044) catches profile/patch rewrites and
  payload substitution. V1 hashes are unchanged.
- Emitter: `active eval --envelope <out.json>` writes the v2 shape for a
  clean run and for parse/budget/validation failures (`payload = null`,
  failed receipt with its class).

Spec `tool-result-envelope.md` and `agent-interchange-profile.md`,
diagnostics E042/E043/E044, the explainer, conformance vectors 0039/0040,
and the core gate/helpers are updated in the same change.

### L-P03/L-P06: hash-pinned includes

`active lock` writes lockfile version 2 with `provenance.include_pins`
(spelling, root-relative identity, SHA-256) and records main/include
paths relative to the lockfile, so a moved project keeps its lock valid.
`active eval` resolves each include against those pins (E041 on a
mismatch; a new edge under an enforced lock is E041), and a v1 lockfile
still parses as legacy, admitting matching content in warn mode (W017).
Documents may also carry the pin themselves:
`include "lib.active" sha256:<hex>` (L-P06). The audit receipt records
`include_resolutions` with the source of every include. Lockfile v1
remains readable; re-run `active lock` to upgrade.

### L-P04: content-addressed include store

`--store <dir>` on `active eval` and `active lock` publishes every
include's source bytes under `<dir>/<sha256>` as an immutable, read-only
entry, deduplicating repeated includes at the storage layer. `active
store add|list|gc` manages the cache; `gc --root <lockfile>` keeps every
digest a lockfile records and deletes the rest (`--dry-run` reports
only). The store is a cache: evaluation still resolves from the declared
paths, and a write failure is the informational W018 with an unchanged tree
and exit status. Root defaults
to `$ACTIVE_STORE_DIR` or `~/.cache/active/store`.

### L-P32: per-evaluation include interning

Within one parse invocation, a filesystem include's source bytes are
read once per canonical path, so a mid-parse rewrite of a path that
still resolves does not change a later include's bytes (a deleted path
still fails E017 and a grown file can still fail the metadata bound).
Its lexed stream is interned by content hash. A repeated include of
identical content reuses the stream instead of lexing again; a host
loader is still called once per resolution. Verification (signatures,
pins, resolutions) still runs per include, the once-per-include wall
check still applies on a cache hit, and the C-002 resolution/byte caps
still bound a fan-out of distinct files. Values, file stamps,
annotations, content hashes, and audit fields are unchanged.

### L-P20: one-pass `active test`

`active test [paths...]` runs schema validation, lint, fmt-check, and
conformance vectors in one pass with one exit code, using only the
shipped core: `.active` files are walked (or named directly), evaluated
hermetically against the empty environment (or `--env KEY=VALUE`),
linted with each file's discovered `.active-lint.toml`, and compared
against the formatter's indent-style canonical output (includes are not
merged, matching `active fmt --check`). A file that needs repair fails
and points at `active repair`. A directory with
`conformance/manifest.toml` runs it automatically; `--conformance` names
one explicitly. Lint findings are notes by default and failures under
`--strict`; schema errors and formatting drift always fail. Empty walks
and empty/malformed manifests fail rather than passing vacuously. The
CLI gains a runtime `toml` dependency (already in the workspace
lockfile); no test framework is added.

### Master Program (CA, SC, R, PE, M, GQ, CAP, CG, D, T2, AK, AUD/OBS)

Additive surface, no version bump: content-addressed Blob values with
BLAKE3 identity and B00x diagnostics (`blob()`, `Blob` schema type,
`--materialise-blobs`), schema evolution metadata and the
`active compatibility` CLI (C04x/E040), multi-step `active repair`
with H006/H008 terminal states, demanded-cone `eval --path` plus FFI
`active_value_project` and LSP incremental memo, Config/Data mode
flag with W013 progressive warnings and E024, decidable graph
queries (`active graph query`), in-document `active_capabilities`
with E025, the capability handshake (`--check-doc`) and sandbox
profiles (`--sandbox`, 4 profiles). Capability descriptor grows 10
→ 13 tools (adds `lint`, `repair`, `compatibility`) plus `profiles`
and new `features` entries; the linter grows to ten rules with JSON
and SARIF 2.1.0 reports; INSPECT v2.

The same program also lands: multi-target code generation validated
against real ecosystems (JSON Schema Draft 2020-12, TypeScript + zod,
Go + validator, Python), Tier-1 packaging made live at 1.4.0 on
crates.io, npm, and PyPI with the universal artifact and SBOMs, the
Tier-2 binding contract formalized, an agent-kit evaluation gate over
ten vectors (including partial-eval cone agreement) with a 14-task
prompt pack, and audit/observability: `active eval --audit-trail`
writes a re-validatable, secret-redacted Active document, tracing is
opt-in and value-free, and Miri covers the trace and blob units.
Consumers pinning exact tool/feature sets should expect additions —
any incompatible change ships under a new major version per the
stability policy (spec/packaging.md §7, docs/version-and-abi-policy).

### Tooling hardening (review rounds)

- **Structural secret taint.** A plain value used as a `secret(...)`
  locator (a `let`, table field, or `env` lookup) and every
  interpolation, reference, or container built from it are material
  even without a `Value::Secret` marker. Step traces, `inspect`, and
  hover share one analysis (`active-core/src/secret.rs`), redact
  known secret substrings longest-first, and report `tainted: true`
  wherever a replacement happened. Containers keep their shape;
  material leaves carry the marker.
- **Audit hashes are canonical.** The audit and `lock` hash is computed
  on a materialised, redacted, payload-free tree, so
  `--materialise-blobs` cannot change a document's identity, and
  `debug --replay` uses the same policy: it reproduces an audit hash
  for payload-resident and `let`-bound secrets. `lock` withholds an
  environment value whose text matches a runtime secret. `budget
  explain`, `viz budget`, `graph query secret-tainted-cone`, and
  `viz taint` absorb the evaluator's seeds (the value-dependent
  queries materialise so a payload-resident secret is seen), and
  inspection's unseeded wrappers re-evaluate hermetically for computed
  secrets while the CLI passes the real seeds. Read-only taint and
  hashing passes materialise through a probe that skips per-alias
  payload accounting, so analysis cannot fail a document that
  exported within its budget; `debug` accepts the same `--max-*`,
  `--budget-profile`, `--resilient`, and `--allow-parent-escapes`
  inputs as `eval`, defaults to strict like `eval`, and renders
  identity-only steps. Replay runs an identity-only wall-bounded
  preflight that mirrors `eval`, then two canonical step/hash passes
  without a wall deadline. `--minimize` parses and evaluates candidates
  under the same mode, profile, parse options, and limits (wall-free
  unless `X002` is the target); it redacts duplicate copies of a secret
  value (another string, a blob payload, a comment, or a base64/hex
  encoding) with exact token matching, keeps canonical env names, and
  fails closed (every string literal) when a hard error stops the walk
  before all secret seeds are collected. Profile-selected `let` and
  computed values at material paths seed the same redactor. The audit document records its resolved
  budget and parse flags (including materialisation; limits above
  `i64::MAX` are refused on the audit path only), so a non-default run
  is self-describing and replayable.
- **Reduction fails closed.** `--minimize` now recognizes dot runs on
  either side of `env`, `secret env(`, and env calls with anything
  past the name (comma or not), and redacts every string literal in
  the document except canonical `secret.env("NAME")` names. The
  reducer also protects those call lines, so a binding literal can
  never be stranded.
- **Value-free type errors.** T001 and quantity messages render kind
  names; repair-browser, LSP, and CLI diagnostics no longer embed
  operands that may be material.
- **LSP correctness.** Formatting parses the buffer's own text with
  includes recorded but unresolved, so format-on-save never inlines
  included keys. The repair action uses the hermetic engine, strict
  budget, and the buffer's file context, so an included schema drives
  the same plan the CLI does.
- **Codegen v3.** Go, Rust, and Python Blob types model the
  `{"$active.blob": {…}}` wire wrapper (TypeScript already did). The
  codegen gate now decodes an actual `active eval` document in every
  target instead of asserting a hand-written shape.
- **Lint JSON stays lint-only.** Parse diagnostics for a broken file
  go to stderr; the pinned `^L[0-9]{3}$` array schema is unchanged and
  the gate covers the broken-file case.
- **Bounded diagnostic output.** Parser and lexer errors are retained
  up to 1024 diagnostics per parse in emission order (lexer errors
  first, then parser errors; the root cause of a single-phase flood
  is first). Beyond that only the pathological tail is dropped; the
  parse still fails, and occurrence reporting below the cap is
  unchanged. Validation (T-code) diagnostics are not capped: every
  violation is still reported.
- **Taint edges** use containment (`a -> a[0]`), never an
  empty-string parent, and inspect provenance keeps real keys such as
  `node_id`. `--max-attempts` requires `--minimize` (clap usage
  error otherwise).
- **Structural taint reaches transforms.** `inspect` resolves table
  fields ahead of `let` names, and maps `@patch` Set targets, profile
  fields, and schema defaults to their expressions, so a value that
  arrived through a transform is tainted exactly like an authored one.
  `viz taint` and the `secret-tainted-cone` graph query use the same
  model, so plain locator sources appear there too.
- **Secret-call spellings are case-insensitive.** `Secret(`,
  `secret.Env(`, and dotted identifier runs are recognized as calls,
  protected from reduction, and redacted fail-closed.
- **Blob identity taint follows the structural model.** Data or media
  material (including an `env` lookup used as a locator) taints the
  blob, refuses materialisation with B003, and redacts `media_type` in
  every export; B004 messages no longer echo the evaluated media type.
- **Redaction markers are opaque.** A secret whose text overlaps
  `***SECRET***` can no longer rewrite the marker.
- **Codegen v3 completion.** JSON Schema and OpenAPI model the
  `$active.blob` wrapper with a closed inner identity; the codegen gate
  rejects inner-only shapes and runs the Rust decode test for real.
  Helper type names are reserved so a user type can never collide.
- **LSP hermeticity.** Non-file buffers never read the working
  directory for includes; the repair action's ambient-environment
  isolation is regression-tested.
- **Structural model completion.** Inspect maps nested transform
  subtrees (a patch table, a nested profile table) and applies
  profiles before patches, matching evaluation order; inactive
  profiles no longer taint. Every `env` read of a secret-bound name
  is material (the lockfile rule, now shared). `let`-bound containers
  keep addressable paths, so step traces and inlay hints mark their
  material leaves. A container report is `tainted: true` whenever a
  child was marked, and viz emits containment edges only between
  emitted nodes.
- **Reduction spellings.** Bare case variants (`Secret(`/`SECRET(`)
  and an env call whose first argument is not a name
  (`secret.env(f(loc))`) are protected and fail closed.
- **Diagnostics.** T006 marks a material actual of any kind; B004
  echoes only the schema-authored expectation.
- **Blob exports.** JSON, YAML, TOML, `Debug`, and inspect share one
  tainted-media rule. The incremental memo hashes media material as
  well as data material, so an LSP edit cannot serve a stale
  untainted identity. Codegen models the optional `payload_b64` on
  every target, and the capability descriptor lists `codegen-rust`.
- **Path analysis is bounded.** A cyclic `let` container
  (`let t = { a = t }`) can no longer grow the structural path map
  without limit; the walk is depth- and size-capped, so every editor
  request terminates. `@rename`/`@remove` participate in the taint
  map, and a computed `active_profile` selector (an env read or
  interpolation) uses a taint-only union of every profile, failing
  closed. W002/W003 never echo a computed selector's value.
- **Lockfiles record blobs.** `active lock` emits a deterministic
  `[[blobs]]` array (path, hash, size, media type, taint) with a
  tainted identity's media type redacted. TOML uses the same nested
  `$active.blob` wrapper as JSON/YAML.
- **Codegen v4.** The optional `payload_b64` field changes every
  target's Blob shape, so the pipeline version is bumped.
- **Transform and selector fidelity.** `@rename` destinations resolve
  in the source parent (with missing sources left untouched), and
  `@remove` shifts array indices in the taint map exactly as the
  evaluator does. A computed `active_profile` selector unions every
  profile's *material paths* into a set, so a later benign profile
  cannot hide an earlier material one; engine profile overrides feed
  the same analysis. Transform maintenance is work-bounded.
- **Truncation fails closed.** When the path walk hits its depth or
  size cap, unknown paths report material rather than benign.
- **Schema-expression diagnostics.** B002/B004 and unknown-format
  T003 echo an expectation only when the schema spells a literal;
  a computed value is never echoed.
- **TOML block records and escaping.** `active lock` documents its
  deterministic `[[blobs]]` records, and the simple TOML literal
  escapes a blob media type as a JSON string (valid TOML).
- **Transform union completeness.** Nested-array `@remove` addresses
  shift in the taint map (`a.b[0]`), and the computed-selector union
  follows `@patch` Set, Remove, and Rename. If transform maintenance
  exceeds its work bound, the map is discarded and unknown paths
  report material (fail closed) instead of trusting stale entries.
- **Computed `requires`.** T004 echoes a `requires` target only when
  the schema spells a literal; a computed target stays value-free.
  The graph `secret-tainted-cone` query honors `--profile`.
- **Structural material stays inside.** `eval`, `convert`, `doc`,
  `lock`, the audit trail, the C ABI, and WASM redact structurally
  material values (plain locator sources, aliases, interpolations,
  containers) before serialization; content hashes are over the
  redacted tree, so a material value change no longer changes the
  hash. Audit results hash the full redacted tree before projection
  and payload stripping, and a strict validation failure writes an
  audit with `result = null` and `outcome = "failed"`.
- **Metadata is immutable after parse.** Patches and profiles cannot
  rewrite `active_mode`, `active_capabilities`, or `active_profile`
  (E019), so the exported marker always matches what the host
  resolved. `active_capabilities` is exempt from a closed root table.
- **Compatibility sees the whole schema.** Nested tables and arrays
  are compared at dotted paths (add/remove/type change), so a removed
  required array or a changed nested field is no longer "no changes".
- **Repair and formatter fidelity.** `--preserve` covers ancestors
  and descendants; partial repair JSON reports only the applied
  subset (unselected steps skipped, verdict re-evaluated); fill
  directives keep `${…}` literal via single quotes and refuse
  unrepresentable control characters; `active fmt` preserves
  `min_reader`/`min_writer` without a version.
- **Editor and query polish.** The `@ai_immutable` action skips
  multi-line string bodies, the mode marker inserts after a BOM,
  capability lists are linear to parse and print capped,
  `inspect`, `schema inspect`, `viz taint`, `viz budget`, and
  `budget explain` accept a `--profile` override (and `graph query`
  accepts it after the subcommand), viz byte sizes count redacted
  strings, and `--browser` rejects flags it cannot honor. The C ABI's
  secret predicate is recursive.
- **Profile overrides keep their material.** A profile may replace a
  locator source with a different value. The selected node now
  inherits the path's materiality, so `eval`, `inspect`, `viz taint`,
  `budget`, `lock`, and the content hash redact the override-selected
  value; before, an override could publish the selected material while
  the base literal stayed redacted. Container replacements mark their
  whole subtree (including a nested replacement under a merged table),
  deep alias chains merge instead of deleting live paths, schema
  defaults join the base material map and register wildcard paths
  (including defaults that contain arrays), and a truncated base walk
  keeps both new and overridden profile paths material. A computed
  selector marks its nodes as well as its paths, so step traces and
  lock provenance agree with the value redactor.
- **Provenance is profile-aware.** `active lock` withholds an env value
  that a profile places at a material path and records its drift hash
  instead (the graph walk alone cannot see the selection).
- **Env fallbacks are mapped.** `env.X ?: {…}` and `env.X ?: […]` keep
  the `Env` node at the path while registering the fallback's children,
  so a material child inside a fallback table or array redacts instead
  of exporting raw. Merge-vs-replace treats a table fallback as a table.
- **Taint paths survive partial walks and namespace collisions.** A
  profile walk that trips the path bound invalidates the overridden
  key as a whole (no partially registered benign base paths), a binding
  name never shadows a schema fill in the export map, and wildcard
  defaults expand per element so an explicitly written array element
  stays visible.
- **Patch paths are canonical and tree-only.** `arr[00]`/`arr[+0]`
  canonicalize to the index the evaluator uses before touching the
  taint map, a rename target stays verbatim (the evaluator accepts
  numeric keys, though such keys are not addressable by later dotted
  patch paths), and a `@rename` whose source is a binding (a W004 no-op
  in the evaluator) no longer moves the destination's materiality.
- **Profile replacements answer for themselves.** A profile value at a
  path covered by an explicit value or fill inherits that path's
  materiality; a profile that supplies a new value (an array element,
  an absent defaulted key) is material only when its own expression
  is. Inherited paths stay path-based, so marking a replacement no
  longer hides an unrelated binding that shares its literal target.
- **Traces and provenance honor path taint.** Debug steps render a
  material profile path with the marker (including internal profile
  containers), while a benign data node shared with that path stays
  visible in its own step; `lock` withholds env reads that are only
  path-tainted. A computed selector's unselected profiles no longer
  count as export or rename candidates, so a rename cannot clear a
  destination's taint and an unselected key cannot suppress a material
  default fill.
- **Redaction is linear, exact, and call-stable.** Redaction compiles
  the secret set into one Aho–Corasick noncontiguous automaton, so a
  scan costs O(text + matches) instead of O(secrets × text): a
  pathological secret set can no longer exhaust a budget and
  wholesale-mark benign values. A pattern longer than 4 KB stays out
  of the automaton and is scanned with `str::find`, so one huge secret
  costs no automaton memory; remaining values live in log-structured
  batches (one matcher per power-of-two group), so an insert costs
  O(1) amortized, a scan visits O(log n) automata, and interleaved
  insert/redact callers never rescan a growing pending set; a failed
  build falls back to a correct linear scan instead of reporting every
  text clean. Matching is leftmost-longest among known secrets,
  existing markers are opaque, a text shorter than the shortest secret
  is rejected in O(1), and repeated exports are byte-identical by
  construction. Profile merging uses a larger, separate bound so
  normal 10k-key profiles do not fail closed. A profile value at a
  material path is resolved statically (literals, `let` references,
  string interpolation, through long alias chains with cycle detection,
  deterministic root order, memoization, and shared node/byte/depth
  budgets that fail the analysis closed when exceeded) and
  seeds the redactor; seeding is deferred until after every profile and
  patch transform, so only a node still visible through a benign
  export path keeps its text readable. Only the selected profile
  contributes (all profiles only for a computed selector, none when no
  profile runs); a computed selector's material paths never enter the
  evaluated-tree map, so their recorded nodes seed directly under the
  conservative union. Memo-served nodes seed
  exactly like fresh ones. A copy of a material value elsewhere (a
  plain string, a comment, or a materialised payload) redacts, and a
  debug bundle keeps include targets, profile names, the
  active-profile selector, and patch path arguments readable even under
  the fail-closed sweep; bytes left between tokens by a healed or
  malformed string are swept too. A value that is exactly a material
  value redacts at any length (whole-value matching), while substring
  spreading stays limited to four bytes and up so a short value cannot
  rewrite unrelated text; constraint sources and T006 actuals (including
  host-only values, redacted before truncation), docgen intent/comment/
  schema prose, LSP hover annotations, L005 migration messages, generated
  code comments, schema inspect fields, and blob media types and payloads
  use the exact set. The
  debug gap sweep touches only true inter-token gaps and redacts a
  malformed interpolation as one slice, so it cannot rewrite identifiers
  or keywords. Runtime secrets seed the same redactor as authored literals: the evaluator returns every
  material node's text (covering `let`-bound and computed secrets and
  lazy host lookups), material environment values are merged, and every
  surface — CLI eval/convert/doc/lock/inspect, FFI, WASM, LSP hover,
  debug replay, and viz taint — absorbs them before rendering. A copy
  in a plain string, a blob media type, or a materialised payload is
  swept; a match drops the payload and marks the identity tainted. Tainted blobs never
  emit `payload_b64` through JSON, TOML, or the redacting `Serialize`
  impl, schema compatibility indexes rows by path (linear instead of
  quadratic), and LSP hover/inlay consult path materiality so a
  profile-selected value is redacted.
- **Analysis and redaction stay bounded.** Profile merging shares one
  work bound across fields and replacements; on exhaustion every node
  and env read is marked material, so paths, traces, and provenance all
  fail closed together. Nested default expansion charges a work budget
  and degrades to a fail-closed wildcard, env-name collection is cached
  per subtree, and redaction uses a single linear automaton, so a
  pathological secret set scans in time proportional to the text
  instead of running for minutes after evaluation.
  Incremental evaluation never memoizes a lazy env fallback answer, and
  the memo config token records the fallback presence.
- **Patch and transform modeling is structural.** A successful
  `@patch` Set registers its destination as a rename/default candidate;
  a failed Set (W004) registers nothing. Computed-selector profile
  paths are materialized before transforms and rewritten by Remove
  (index shift) and Rename, and schema defaults expand over containers
  introduced by profiles or successful Sets, per element.
- **Traces and materialisation follow path taint.** A step whose
  rendered path is material reports `tainted: true`, including a
  profile override of a base data path; the trace overlay follows
  same-parent renames, array-element no-ops, and index shifts after a
  removal. `--materialise-blobs` runs the demand on the redacted cone,
  so a node- or path-material blob fails closed with B003 instead of
  silently dropping its payload.
- **Env materiality follows the name.** An env read placed at a
  material profile path makes every read of that name material,
  including reads nested under interpolations, operators, quantities,
  blobs, and env fallbacks; the upward expression closure is extended
  after the profile so embedded strings and containers redact. A
  computed selector's runtime rename carries the source's materiality
  to the same-parent destination (`a.x` → `zz` marks `a.zz`). `inspect`
  treats a path-material blob as tainted and redacts its media type,
  and the step-path overlay follows Remove/Rename with the same-parent
  rule. A step whose own path is material reports `tainted: true`, so
  `--break taint` still selects it.
- **Material values never auto-rename.** Resilient H005 skips a source
  path that is structurally material: the taint map follows the source
  path, and moving the value would publish it. The typo degrades to a
  suggestion-only T005. Binding names no longer pollute data-path
  materiality, and a default's materiality applies only when the
  default can actually fill (an explicit value wins). A canceled fill
  may keep the healed key conservatively marked (hidden, never
  exposed).
- **Metadata is immutable at every depth.** Patches targeting
  `schema.x`, `active_mode.x`, `active_capabilities.x`, or
  `active_profile.x` are rejected with E019, so descendants cannot
  fabricate the metadata table the host resolved.
- **Editor actions respect physical strings.** The `@ai_immutable`
  action skips continuation lines of plain `"…"`/`'…'` strings as well
  as triple-quoted bodies, and zero-width insertions land after a BOM.
- **Repair never suggests metadata fills.** A schema default on a root
  metadata key is excluded from the plan (the engine rejects it), so
  every reported step is one the applier accepts.
- **Compatibility covers requiredness and element blobs.** Removing a
  default (optional → required) is C042, `Array<Blob>` element bounds
  and media are compared at `path[]` (nested arrays at `path[][]`), and
  newly pinning `max_size` is a breaking tightening.
- **Inspect handles nested arrays.** Every bracket index descends one
  schema level, so `arr[0][0].a` reports `Int`, not `Table`.
- **Audit paths are inert.** `$` is escaped in audit strings, so a
  source path containing `${…}` re-validates instead of interpolating.
- **Audit replay is complete.** The audit trail records the effective
  `profile` selector, `debug --replay` takes `--profile` and feeds it
  to both evaluation and redaction, and a strict validation failure
  writes `result = null` with `outcome = "failed"` (spec/audit-trail.md
  §1–2); other fatal engine errors carry no audit, as documented.
- **Repair verdict paths.** Partial-repair `final_codes` keep the
  `CODE@path` shape from the full plan, including validation
  diagnostics surfaced through `EvalError::ValidationFailed`.

### SOLID → Active rename

The language is renamed SOLID → Active across code, packages, docs,
and fixtures: crates `active`, `active-core`, `active-cli`,
`active-fmt`, `active-lsp` (+ `active-fuzz` harness); binaries
`active`, `active-fmt`, `active-lsp`; library `active_core`
(`ActiveType`, …) with the `active` facade crate; file extension
`.solid` → `.active`; editors, workflows, and release artifacts
follow. GitHub URLs and prior release notes keep their original
names (redirects/history).

## [1.4.0] — 2026-10-20

Embedding release, published to crates.io (`active`, `active-core`,
`active-cli`, `active-fmt`, `active-lsp`, plus first publishes of
`active-ffi` and `active-wasm`). New public crates are new
functionality, backward compatible: the frozen C ABI (opaque handles,
structured diagnostics, budgets, secret discipline, cbindgen header
with CI drift check) and the zero-import WebAssembly module (browsers,
Node, Deno, Bun via the JS/TS shim; Go over wazero with no cgo),
both running the same conformance manifest as the reference
implementation. Also aboard: the Miri-hardened wall-clock gate and
single-ownership FFI test host (full Miri 21 of 21 clean, zero
findings), the WASM-powered playground with headless-browser proof,
the enterprise README rewrite with Figma-composed banner and graphics,
and the secret taint plus dependency graph documentation. No language
changes: the 1.0 surface stays frozen.

## [1.3.0] — 2026-10-18

Units Phases B+C (AI-age trio + domain SI): token counts
(`tokens ktokens Mtokens`, glued spelling only), inference throughput
(`tok/s`), compute (`FLOPS TFLOPS PFLOPS`); pressure (`Pa bar psi`),
electrical (`V A W`, cross-dimension refuses), energy (`J kWh`),
force (`N`); angles (`deg rad`, radians canonical), rotation (`rpm`
retained); length (`meter km`, long-form only — `m` stays minutes);
mass (`kg g`); large bytes (`PiB EiB`, saturating); temperature
(`K Cel Fahr`, affine kelvin canonical, parser folds unary minus into
affine literals so the -40 crossover holds) and decibels (`dB`,
ordering-only — affine/log arithmetic refused). Same-suffix arithmetic
for all retained quantities. No changes to any previously-parseable
source (full suite green throughout).

## [1.2.0] — 2026-10-18

Units Phase A (Tier 0 gaps): percent `%` (dimensionless ratio, 100% = 1.0,
percent-form export, no implicit base, no clamping); frequency
`Hz kHz MHz GHz THz` (f64 hertz canonical, full arithmetic, case-sensitive
SI spellings); bits + data rates `b Kb Mb Gb bps Kbps Mbps Gbps MB/s`
(canonical bits / bits-sec, `MB/s` converts through megabytes, bits-bytes
mixing refused); `week` (exact 604800000ms) + `sec`/`min` long aliases
(canonical `s`/`m` respell; plurals refused); same-suffix arithmetic for
all retained quantities (rates included), cross-suffix stays dimensional
TypeError; explicit unary negation preserved on quantities. No changes to
any previously-parseable source (full suite green throughout).

## [1.1.0] — 2026-10-17

Hardening release, published to crates.io (`active`, `active-core`,
`active-cli`, `active-fmt`, `active-lsp`). No language changes:
the 1.0 surface is frozen. Tested brutally since 1.0.0 — lexer/parser
fuzz harness (no panics, no hangs), property-test invariants
(determinism, cycle detection, secret non-leakage), execution
budgets, and a 1500+ case research battery pinning YAML/TOML/JSON
failure lineages to diagnostics. Repository home moves to
`Active-Official/active` (mirror); personal repo renamed
`RatioArtificiosa/active-lang` (redirects preserved).

## [1.0.0] — 2026-10-16

First stable release. The C-090 readiness review
(spec/readiness-1.0.md) passes all ten success criteria; the
checklist (C-010 through C-090 plus C-070) is complete; the
conformance suite is green and this repository is the declared
reference implementation.

**Never published.** The `v1.0.0` tag exists and this entry declared it the
first stable release, but no registry ever carried 1.0.0 (crates.io jumps
0.1.0 -> 1.1.0, GitHub releases start at v1.1.0, PyPI/npm have no 1.0.0).
The first installable stable 1.x was 1.1.0. Do not retro-publish 1.0.0:
the code has moved on and a stale artifact would confuse the record.

Since 0.1.0-alpha.1: JSON keys serialize in source binding order
(`preserve_order`; conformance sidecars re-emitted), comment
attachment is file-aware across includes, schema entries take
per-field documentation comments, `solid doc` accepts
`--resilient`/`--allow-parent-escapes`, and the docs site gained
Terraform/CI cookbook entries.

## [0.1.0-alpha.1] — 2026-10-16

First public alpha of the complete roadmap state.

- Language: dual syntax (braces, indentation, mixed, dotted
  headers), interpolation with dependency edges, multi-line strings
  and semantic blocks, literal-path includes, profiles, structural
  patches, full schema language with strict/resilient validation,
  annotations (`@intent`, `@constraint`, `@ai_immutable`),
  transitive secret taint, execution budgets.
- Healing: three-phase engine (syntactic, schema-key H005,
  type-coercion H007) with the published confidence policy.
- Tooling: `solid` CLI (eval, check, graph, impact, why, convert,
  import, generate, doc, lock, keygen, sign, verify, fmt, lint),
  `solid-fmt`, `solid-ls` (LSP 3.17 over stdio), JSON/YAML/TOML
  converters, schema codegen (JSON Schema, OpenAPI, TypeScript, Go,
  Rust serde), Markdown/HTML/man docgen, Mermaid/DOT/JSON graphs.
- Reproducibility: hermetic evaluation, SHA-256 redacted-tree
  hashing, TOML lockfiles, Ed25519 signed overlays.
- Performance: 10k-node eval under 50 ms p99 with fingerprint
  incremental re-evaluation; CodSpeed-tracked criterion benches.
- Distribution: `solid-lang-core`, `solid-cli`, `solid-fmt`,
  `solid-ls` packaging metadata; 6-target release matrix; VS Code
  extension; Neovim plugin; standard schema library
  (`schemas/`); mdBook documentation site.
- Conformance: `tests/spec/` (13 pairs) is the official suite; this
  repository is the reference implementation for 1.0.

## [0.1.0] - 2026-10-16

**Published without a source commit.** crates.io carries 0.1.0 for the five
main crates, but no commit ever set the workspace version to plain 0.1.0:
the publish came from an uncommitted tree between `e680645` and `d5de9b3`
(both `0.1.0-alpha.1`). The version stays published (crates.io versions are
permanent, and it has users); it is not reproducible and is not the
reference for any behavior. `release.yml` now runs a provenance gate
(LANG-J-001) that fails a release whose tree is dirty, whose commit is not
the tag, or whose workspace version differs from the tag, so this cannot
recur.

### L-P47: hash-addressed playground snippets

A playground snippet can be shared as `#blake3:<hash>:<base64url source>`
so the link is its content: the page hashes the bytes and refuses on any
mismatch before showing or evaluating anything. A hash-only
`#blake3:<hash>` link resolves from a bounded local cache and is
re-verified on every read; a miss is a visible refusal, never a network
fetch. The self-contained form is the portable share; the cache (64
entries / 1 MiB, user-clearable) is per browser. Evaluation results are
cached in memory per session keyed by the hash, so re-opening the same
snippet does not re-evaluate it. There is no server storage, no upload
endpoint, and no pastebin: the URL carries the content, or the visitor's
own cache does.

The engine gained a BLAKE3 export (`active_blake3`), the shim a
`hash(source)` method (a stale module fails loudly), and the page a
"Share by hash" and "Clear cached snippets" control. The full scheme,
verification order, retention, and abuse rules are in `spec/playground.md`;
`playground/test-snippets.js` gates the logic against the real release
wasm in CI (round-trip, match, mismatch refusal, tampered-cache refusal,
cache bound, result cache). This lands after G-023 (`4800d2f`), whose
release-shaped playground bundle was served and exercised in a browser.

Follow-up hardening after review: the result cache stores a detached,
deeply frozen copy and caps the whole record (including `items`); both
compression directions use `pipeThrough` (a manual writer awaited before
the readable is consumed can deadlock on backpressure); a 404 example
cannot become editor content; 1- and 2-byte snippet links round-trip
(`atob` padding); cache enumeration ignores non-hash keys; and the
loader reports an HTTP status before the byte-source check. The freeze
is depth-unbounded (a 12-deep items chain is frozen), both decoders
refuse a base64 length `% 4 == 1`, `cacheClear` keeps non-hash keys, the
decompression reader is released on every exit, and the loader accepts
cross-realm buffers and views.

### L-P18: Pulumi, Terraform, and CDK generate targets (codegen half)

`active generate` speaks to the three IaC consumer tools while Active
stays the source of truth:

- `--to terraform|tf` emits an HCL `variables.tf`: one variable per root
  field with its native type, description, static default, `sensitive`
  for secrets, and validation blocks for integer-ness, bounds, string
  lengths, patterns, formats, enums, null-only fields, closed empty
  tables, and scalar-array element constraints. Duration and Bytes are
  canonical integer milliseconds/bytes, with an integrality check. The
  output is `terraform fmt`-canonical and `terraform validate`-clean; the
  gate also proves plan-time acceptance of valid values and rejection of
  out-of-range, wrong-enum, bad-format, fractional-duration, and
  extra-key values.
- Terraform refuses what it cannot state faithfully: a **structured
  table** (a `terraform plan` probe on 1.9.8 shows object conversion
  silently discards attributes outside the type, so the binding would
  drop keys), a `Blob`, a `OneOf`, a `requires`, an array of structured
  tables, a nested array, an array of Blobs or unions, a computed
  bound/length/pattern/format/default, and anything material (enum
  member, pattern, format, or default). HCL template sequences in
  authored prose or defaults (`${`, `%{`) are doubled (`$${`, `%%{`) so
  they render literally instead of interpolating; multi-line
  descriptions keep their line breaks. An array of empty closed tables
  validates every element. Refusals are hard errors naming the field —
  never silent drops. The codegen gate pins the whole refusal surface
  and verifies Terraform 1.9.8; it fails closed when Terraform is absent
  (an explicit `ACTIVE_ALLOW_NO_TERRAFORM=1` opt-out prints a visible
  SKIP for a documented structural run).
- `--to pulumi` and `--to cdk` emit the proven TypeScript interfaces +
  zod schema plus a native reader (`readPulumiConfig` on
  `pulumi.Config.getObject`, `readCdkContext` on
  `construct.node.tryGetContext`) that validates the `"active"` object
  before the program sees it. They inherit the TypeScript refusals.
- The zod target now uses Active's own anchored regex for every named
  format instead of zod's `.email()`/`.url()`/`.uuid()`: those are
  narrower (zod's email TLD rule rejects `a@b.c`, which Active accepts),
  so a binding built on them rejected valid documents. One regex source
  now drives JSON Schema, Terraform, and Zod.
- Two generator gaps surfaced while proving the targets: a table/array
  default (`Any(default = { flag = true })`) is a value expression, not a
  literal node, so it was treated as dynamic and its field as required;
  `static_value` now builds the static aggregate, the JSON Schema and Zod
  targets emit it, and the field is optional. `Any` fields also returned
  early with `true`, silently dropping their default/description/
  deprecated annotations; they now emit an empty schema object plus the
  shared annotations. Zod defaults are type-directed (a Duration default
  uses its canonical `ms` number, not the wire wrapper).
- No diagnostic code is minted: codegen refusals are hard `Err`s naming
  the field/feature, matching the existing Go/Rust refusal surface.
- The codegen gate grows: generated Pulumi/CDK modules are compiled with
  `tsc --strict` and run against the same valid/invalid values as the
  zod target; Terraform is `fmt`/`validate`/plan-tested; CI installs
  Terraform 1.9.8 for the gate. Spec `spec/converters.md` §7 records the
  mappings, refusals, and the regenerate-never-commit workflow.


### L-P44: units as types (dimensional analysis)

Dimensional safety is a named diagnostic on every arithmetic operation.
The evaluator's type step now reports `dimension mismatch: Add combines
quantity:V and quantity:A` (both dimensions named) instead of a generic
"unsupported binary op", and absolute temperature or decibels refuse
arithmetic with the rule named (`affine quantities have no arithmetic`)
while equality and ordering still work through the canonical form.

Negative duration results are T001, never clamped: `1s - 2s`,
`1s * -1`, and `1s / -2` used to silently produce `0ms` — the B-011
failure on the arithmetic surface, where an authoring mistake became
"expires immediately". The B-011 pin (negative durations are rejected,
`heal.rs:258-263`) now governs results as well as coercions; zero stays a
valid duration and addition overflow keeps its documented saturation
policy.

A scalar divided by a duration is the per-second rate (`1024B / 1s` →
`1024.0`; bytes are `Int` at evaluation, so this is the bytes/duration
rule). Division by zero names itself for durations, quantities, and a
scalar over a zero duration; the affine rule wins where both apply
(`20Cel / 0` is an affine-temperature error). Negating an absolute
temperature below 0 K is T001, and `-0K` exports as canonical `0.0 K`;
the parser's folding keeps valid negative Celsius/Fahrenheit readings.
`-0s` is zero, and decibel negation (`-3dB`, attenuation) is preserved.
A NaN factor is reported as NaN, never as a negative duration; duration
scaling overflow keeps the documented saturation policy, and integral
factors take an exact integer path. A folded `Cel`/`Fahr` literal below
0 K (`-300Cel`) is T001 rather than clamped to absolute zero, matching
the `-5K` rule. Negative zero is canonicalized to `0.0` in every numeric
dimension (one zero, one content hash). T001 is reused — its published
meaning already covers a dimension mismatch — so no diagnostic code is
minted. The rules live in the type-checking step of evaluation, the only
layer that sees operand dimensions (conflict note in
`audit-pack/DESIGN-L-P44.md` for Bud); the schema layer keeps
value-vs-bound dimension errors (T003).

Spec `specification.md` §4.1/§4.3 and `validation.md` §10.2 record the
table and the pin; tests in `active-core/tests/dimensions.rs` and the
evaluator's own unit tests cover each operation, the named errors, the
zero cases, and the unchanged values for dimension-correct documents;
conformance vectors 0062–0069.

### L-P25: generated contracts proven by differential test

The JSON Schema generator is now proven to be the contract, not a sketch.
`active-core/tests/generated_contracts.rs` walks every schema-bearing
`.active` file in the repository corpus (conformance cases, examples, the
standard schema library, docs-site, tests, agent-kit vectors, fixtures),
renders each schema back to Active source, generates boundary/omission/
extra-key/`requires` candidates from the schema tree (the deterministic
L-P66 family discipline), evaluates each through the shipped resilient
engine, and validates the wire JSON against the generated schema with the
pinned Python `jsonschema==4.26.0` already used by the codegen and
output-schema gates (an external batch check; the Rust dependency graph is
unchanged, and no network crates enter it). A verdict disagreement fails
the test; the Active schema is never loosened. 91 schemas and 1843
candidates pass with zero divergences; a coverage gate requires the
exercised schemas to remain the majority of the schema-bearing corpus,
so a growing pile of skips fails the test.

The differential test exposed and this change fixes the live contract
gaps: the schema now declares its dialect (`$schema: draft-07`);
disjunctions emit `anyOf` (Active accepts any branch; `oneOf` rejected an
`Int` of an `Int|Float`); `requires` emits the full biconditional as
`allOf` conditionals (forward enablement + the reverse T010 rule) instead
of a forward-only `dependentRequired` that ignored disabled triggers;
conditional fields are no longer in the unconditional `required` array
(the trigger-disabled T004 excuse) and every binding target (TypeScript,
Zod, Go, Rust, Python) types them optional; named formats emit Active's
own regex as `pattern` so `uuid`/`semver` are enforced where `format` is
annotation-only; blob `max_size` is a validating keyword and the wire
`media_type` accepts both the authored type and the redaction marker a
tainted identity exports; and a duration's `ms` magnitude carries
`minimum: 0`, so a negative duration no longer validates. Enablement
candidates for an `Any`/`OneOf` trigger cover every disabled literal
plus the zero-quantity wire shape (`0%`/`0Hz`), closing a fail-open
case. A closed root contract allows the document metadata keys the
validator exempts.

Review round two closed five more: a `requires` target spelled through a
`let` binding now resolves to the same conditionals as a literal, and a
target computed at evaluation time is listed under
`x-active-requires-computed` and counted instead of silently accepted;
the blob media `enum` accepts the tainted marker; the `Any` trigger's
zero-quantity shape; a statically known bound the engine cannot order
(`Int` min `50%`, float `Bytes`/`Duration` bounds, unit `min_len`)
renders an unsatisfiable field (`not: {}` with a `$comment`) instead of
a bound Active never enforces; and a unit-spelled static default
(`30s`, `1KiB`) renders its wire value and no longer makes the field
`required`.

Review round three closed the remaining boundary gaps: `let`-spelled
numeric bounds and unit blob `max_size` now reach every target (Go tags,
zod, JSON Schema) instead of being treated as dynamic; a dangling
literal `requires` target makes its table unsatisfiable (`not: {}`),
matching the T003 evaluation reports for every document; TypeScript/Zod,
Go, Rust, and Python refuse a schema with an unsatisfiable field (the
JSON Schema target states it exactly, including an unknown format name,
a static non-string pattern/format/media_type, and a negative or
non-integer blob `max_size`); Go carries Duration/Bytes bounds as
validator tags and zod carries the blob size/media constraints; the
harness walks `OneOf` branch tables for `requires` and bounds, includes
dangling-target and binding-bound fixtures, and its coverage commentary
records why parse-negative fixtures stay outside the ratio. Dotted or
root-absolute `requires` targets stay annotated `x-active-requires` and
counted as known limitations. Spec `spec/converters.md`,
`spec/schema-extensions.md` §4; conformance vectors 0059–0061 (Bytes
bound, Duration bound, nested requires-disabled T010).

### L-P41: patch and repair commits are atomic, with receipts

Patching is now validate-then-commit. The engine's `apply_patches` applies
the whole directive set to a staging clone and installs it only when every
directive succeeds; a fatal directive (metadata rewrite, immutable target,
invalid path, rename collision) leaves the caller's tree byte-identical.
A blocked or missing target stays W004 (a skip, not a failing directive).
A nested key typo now emits `@rename("server.old", "new")` — the parser's
plain-leaf form — instead of a dotted target the parser refused, so nested
typos reach clean; the full endpoint stays in the plan metadata, and the
destination/metadata/immutable checks resolve it under the source's parent
(which also closes the rename-to-absent-immutable-path gap).
`active repair --output` publishes through the atomic same-directory
temp-and-rename path: a refusal or graceful abort leaves the destination
byte-identical and no `.tmp` behind (a hard kill can leave a uniquely
named temp, never a partial destination). `--receipt` writes an L-T06 envelope
v2 (also atomically) on success and on refusal: the payload is the
structural applied set plus source/result hashes, the receipt carries
`applied` and `document_clean`, and directives/values never appear (a
directive can carry a value). A refused repair writes a failed receipt
with the attempted set and a failure class, and no output.

`active apply`'s plan receipt gains `steps` (the applied set);
a refused apply writes a `refusal` receipt (applied false, the stable
`[class]`, and the plan hash/steps only after the MAC and approval
verified) and never touches the baseline. All receipt writers now publish
atomically, including `write_envelope_v2`. The plan-receipt schema is
updated and the output-schema gate validates both the success and refusal
shapes. Honest scope: every shipped commit is single-file; no multi-file
overlay writer exists, so multi-file atomicity is not claimed (a future
overlay needs a two-phase journal). Spec repair §3.1, reproducibility §4e,
tool-result-envelope §1.1, audit-trail; docs/tooling; CLI + core tests and
three negative controls.

### L-P14: evaluated values are frozen at the boundary; determinism is D-1

`spec/reproducibility.md` §1.2 names the determinism property **D-1**: for
a fixed input tuple (source and include bytes, injected environment,
engine configuration, evaluator build), evaluation produces byte-identical
canonical JSON, identical diagnostics, and the same `content_hash` /
`bound_content_hash`; wall time may refuse a run but never changes a
value. A dedicated `determinism.rs` test pins the property for tables,
strict mode, injected env, redacted secrets, profiles, and fresh engines.
The `content_hash` canonicaliser now sorts object keys recursively, as
§3 has always specified (`serde_json` runs with `preserve_order`, so the
old code followed source order); the same table in another key order now
hashes identically, the hash stays over the redacted tree, and the two
shipped example envelopes whose payloads were not already sorted carry
refreshed hashes.
The boundary is now explicit: the C ABI value handle is opaque and every
reader takes `const` (only `active_value_free` takes a mutable pointer, for
ownership release); a header-surface test allowlists every `ActiveValue`
export and a runtime test proves mutating the returned JSON copy cannot
change the handle. The WASM JS shim deep-freezes its result record and
diagnostic items (asserted by the conformance runner), and its typings are
readonly. No export computes a hash over a host-owned mutable object, so a
host that copies and mutates a value holds a different value. Docs
(`docs/FFI.md`, `docs/bindings.md`), spec, and the boundary tests ship
together; no header change.

### L-P11: closing a table is an auditable promise

`active compatibility` now reports every `closed` flag change as C046 at
error severity, in both directions: closing rejects keys that were
addable, and opening withdraws the "no extra keys" promise readers may
rely on (a widening that adds or withdraws a promise is no longer treated
as a compatible branch relaxation). The schema root and disjunction
branches are covered; a close/open inside a `OneOf` is one C046, not a
removed branch. A clean L-T06 receipt gains `closed_tables`: every closed
schema table instance with its dotted document path and the key names
present at it (sorted, names only — never values or material, and the
walk runs over the redacted document). The member binds through
`receipt_digest`. Closing never changes the evaluated value. Surfaces
agree: the formatter renders and round-trips the flag, the validator owns
T005, codegen emits `additionalProperties: false`, and lint stays silent
(T005 is total; a lint duplicate would double-report). Spec validation
§10, schema-evolution §2, tool-result-envelope §1.1, diagnostics/explain,
conformance vector 0058.

### L-P10: schema disjunctions and the `requires` biconditional

Schema fields gain `OneOf(A, B, ...)`: a disjunction whose branches are
full schema types. A value is accepted when any branch accepts it (each
branch's constraints apply, and branch trials are transactional); a value
failing every branch is one T002 naming the branch list and up to four
branch reasons. Coercion needs exactly one coercible branch; two are
ambiguous. A default is the weakest disjunct, fills only when no source
provides the key, and must satisfy a branch — checked on every evaluation
in both modes. `requires` is now a biconditional: a present field with a
disabled trigger is the new T010, while B-001's absent-field excuse is
unchanged. JSON Schema and OpenAPI render `oneOf` with per-branch
constraints and the default; TypeScript, Zod, and Python render unions;
Go and Rust refuse instead of widening the field. `explain` gains T006 (a
pre-existing gap) and T010. Spec validation §3/§5/§9, grammar §2.10,
schema-evolution, converters; conformance vectors 0049–0053.

### L-P05: `eval --pure` (an enforced purity mode)

`active eval --pure` refuses ambient input before evaluation: any
`env.NAME` read (including a `secret.env` locator), any include without a
content pin, and any profile override is E047, fail-closed. The check
lives in the engine (`Engine.pure`), so FFI/WASM hosts get the same
refusal; the document's own profile block and `active_profile` key stay
allowed, and an injected environment snapshot is refused too. A pure
document evaluates identically with and without the mode (same bytes,
same content hash); default evaluation is unchanged. Spec
`spec/reproducibility.md` §1.1, `docs/tooling.md`, diagnostics/explain,
vectors 0054–0057 (the shim harnesses are pinned until they expose the
mode).

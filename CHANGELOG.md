# Changelog

All notable changes to the Active language, toolchain, and
specification. (Entries below the rename keep their original
SOLID-era names: that history is accurate as written.) Versions
follow SemVer with a unified workspace version (spec/packaging.md §1).

## [Unreleased]

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
  wholesale-mark benign values, and a very long secret builds a
  bounded automaton instead of enumerating a dense byte-per-state
  table. Matching is leftmost-first, existing markers are opaque, and
  an over-long candidate is rejected on its length in O(1) before the
  automaton is built. Repeated exports are byte-identical by
  construction. Profile merging uses a larger, separate bound so
  normal 10k-key profiles do not fail closed. Blob media types and
  payloads are swept for authored secret literals (a match drops the
  payload and marks the identity), tainted blobs never emit
  `payload_b64` through JSON, TOML, or the redacting `Serialize` impl,
  inspection redacts a materialised payload the same way, schema
  compatibility indexes rows by path (linear instead of quadratic),
  and LSP hover/inlay consult path materiality so a profile-selected
  value is redacted.
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

# Hermetic Evaluation & Reproducible Builds (C-050/C-051, P-050)

Evaluation is a pure function of declared inputs. Lockfiles bind a
value tree to its inputs, toolchain, and content hash for audit
and reproducible deployment.

## 1. Hermeticity: the closed world

An evaluation may depend on exactly four things:

1. The parsed module (source text + include graph, resolved at
   parse time — never during evaluation).
2. The injected environment snapshot (`Engine.env`).
3. The engine configuration (resilient flag, profile override,
   budgets).
4. Wall-clock time, for budget enforcement only: it can fail a
   run, never change a value.

Enforcement points (audited, tested):

- The evaluator performs no I/O: no filesystem, no processes, no
  network, no ambient environment reads. The single former
  exception (`std::env::var` in the `Env` arm) now reads the
  injected snapshot. `grep std::env / fs / process / net` over
  `active-core/src/eval` must stay empty (a test asserts the
  snapshot is honored and ambient variables are invisible).
- `Engine::new()` snapshots the process environment (CLI
  behavior, backward compatible); `Engine::with_env(map)` injects
  a closed snapshot (tests, agents, reproducible runs).
- Includes resolve at parse time against explicit base paths with
  escape/cycle guards (C-025); evaluation never opens files.
- Budgets bound every dimension (C-037); unbounded work cannot
  hang a deployment.

### 1.1 `--pure`: the promise as a mode (L-P05)

`--pure` turns the closed-world rule into an enforced mode. Before the
walk, the engine refuses (E047, fail-closed) when the module or the host
supplies any of:

- an `env.NAME` read anywhere in the arena (including a `secret.env`
  locator), naming the variable. An injected snapshot is refused too:
  purity means the document alone, not "a closed snapshot existed";
- an include whose recorded resolution is unpinned (`Unpinned`,
  L-P03/L-P06). Parse still verifies pins (E041); purity additionally
  demands that one existed;
- a host-selected profile (`Engine.profile_override`, `--profile`). The
  document's own `profile` block and `active_profile` key are document
  input and stay allowed.

A pure document evaluates identically with and without the flag: same
value bytes and same `content_hash`. The default (non-pure) evaluation
is unchanged, and the check is a structural pre-scan, so dead code in an
unselected profile cannot hide an ambient read either.

### 1.2 Determinism (D-1): the testable contract (L-P14)

**D-1.** For a fixed input tuple, evaluation is byte-deterministic. The
tuple is:

- the parsed module: source bytes plus every include's bytes and pin;
- the injected environment snapshot (`Engine.env`);
- the engine configuration: mode, resilient flag, profile override,
  budgets, reader version, and the pure flag (L-P05);
- the evaluator build.

Given the same tuple, two runs produce:

- byte-identical canonical JSON for the value (`value_to_pretty_json`,
  and the redacted form when a secret profile applies);
- identical diagnostics: the same codes, severities, paths, spans, and
  order (G01). Message text is human-facing and may differ between
  implementations unless this spec defines a template; the differential
  and the conformance harness compare the structured record, never the
  prose;
- the same `content_hash` over the redacted value (the §3 canonical form,
  keys sorted), and the same `bound_content_hash` for the same schema
  version, profile, resolved budget, mode, and capability.

**Diagnostic order (G02).** Diagnostics are emitted in this phase order:
parse (emission order, which is span order for the left-to-right parser)
-> evaluation (topological walk order) -> profile/patch (source order)
-> validation (schema field order, then T010 at the field, then T009 in
declaration order) -> post-evaluation envelope checks. Within a phase,
ties break by `(path, code, span)`. One condition produces one
diagnostic. The conformance engines compare the ordered list.

**Diagnostic spans (G03).** Parse diagnostics carry the producing token
or statement span. Validation follows `spec/validation.md` §8. T001 and
T006 use the operator or predicate span; W001 the identifier token; W005
the schema field; X-codes the offending node when attributable and the
document root otherwise; E029 the file; W013 has `span = null`; S004
carries no span by design. When a value's own span is unavailable (a
patched or profile-merged value), the schema field or the patch
instruction span is used.

Wall-clock time may **refuse** a run (a budget trip, X00x) but never
changes a value it produces. Time, randomness, locale, host paths, and
the ambient environment are not inputs: §1 already makes the evaluator
I/O-free and the snapshot the only ambient source. A pure evaluation
(§1.1) is a special case with a narrower tuple.

**Where the promise ends.** The language computes the hash over its own
evaluated tree. Values cross the embedding boundary as immutable views:
the C ABI returns an owned, opaque handle whose every reader takes a
`const` pointer (only `active_value_free` takes a mutable pointer, and
it releases ownership without reading or changing the value), and the
WASM boundary returns JSON bytes plus a JS record the shim freezes. A
host that copies a value and mutates the copy holds a *different* value;
re-hashing that copy is not the language's result. No shipped API hashes
a host-owned mutable object.

Enforcement: `active-core/tests/determinism.rs` (the named property) and
the multi-embedding conformance harness (every vector runs through the
reference core, the C ABI, WASM, and native bindings).

## 2. Provenance manifest

Collected post-evaluation from the module, the snapshot, and the
filesystem (for hashes only):

- **env**: every `Env` node in the arena (declared inputs — a
  superset of attempted reads, the audit-safe direction), each
  with `name`, `set`, and either the value or a secret record:
  - plain reads (`env.NAME`): the value is recorded verbatim,
    per the checklist ("record the values").
  - secret-bound reads: any variable reachable backward through
    the dependency graph from a `Secret` node — direct locators
    (`secret(env.X)`, `secret.env("X")`) and transitive flows
    (`let h = env.H` + `secret(h)`) alike — plus a read that a
    profile places at a material path and a read whose *value*
    matches an evaluator secret seed (a copy of a `let`-bound or
    computed secret). No value is recorded, only `secret = true`
    plus `value_hash = sha256:<salt-hex>:<hex>` for drift detection
    and `value_salt = <salt-hex>` beside it (LANG-I-001: the digest is
    over the salt bytes followed by the value). A salt is public by
    design and defeats precomputed tables and cross-lockfile
    correlation, never a targeted guess at a low-entropy value:
    treat a lockfile carrying `value_hash` as sensitive. Re-locking to
    the same output file reuses the stored salt (identical bytes); a
    fresh lockfile gets a fresh salt. A
    variable that feeds a secret anywhere is secret everywhere
    (even its plain uses withhold the value). The secret invariant
    (C-036) overrides the record-the-values rule here: a manifest
    must never carry live secrets.
  - unset vars record `set = false` (replay detects flips).
- **includes**: every distinct file stamped in `node_files`
  except the main file (canonical-identity comparison), each
  with its stamped path and `sha256` content hash. Files that
  vanish between parse and lock are hard errors (provenance
  cannot be proven).
- **include_pins**: one entry per resolved include, `as` (the spelling the
  importing document used), `resolved` (the path the bytes were read from,
  relative to the root document's directory with `/` separators and `..`
  segments for an include outside it; absolute only when the two share no
  root), and `sha256` (the digest of those bytes). `includes` is keyed by
  the lockfile frame, which suits drift checking; `include_pins` is what
  eval-time resolution matches against, preferring `resolved` so one
  spelling used from several directories stays unambiguous (L-P03). A
  moved tree keeps the same root-relative identities; a renamed include
  with identical content resolves only in warn mode against a lockfile
  that predates identity pins.
- **main**: the main file's path and content hash (when the
  parse was file-based).
- **profile**: the effective profile name when one is active
  (selection changes values — it is an input).

## 3. Content hashing

`content_hash = sha256:<hex>` over the canonical redacted form:
compact JSON with keys sorted recursively by the shared `canonical_json`
writer (the same writer the bound hash and the receipt digest use;
`serde_json` runs with `preserve_order`, so a plain `to_string` would
follow insertion order — the canonical form never does), secrets already
replaced by `***SECRET***`
(`value_to_json`), retained blob payloads stripped first.
Hashing the redacted, payload-free tree (never raw secrets)
keeps lockfiles publishable and makes the identity independent of
`--materialise-blobs`. The audit and replay use the same
canonicaliser, so `debug --replay --expect` reproduces an audit
`result`. Same tree → same hash on every platform (tested
cross-fixture; CI runs both).

**Canonical scalar spelling (G13).** Floats use the shipped writer's
shortest round-trip form, exactly: a positive exponent carries a plus
sign (`1e+21`); a negative exponent does not (`1e-7`); integral values
in ordinary range keep `.0`; `-0.0` is rendered `0.0` (the sign of zero
is not part of the canonical text); the digits are the shortest sequence
that round-trips. Non-finite values never come from source (B-009); a
host-held one exports as exactly `{"$active.float":"inf"}`,
`{"$active.float":"-inf"}`, or `{"$active.float":"nan"}` and hashes in
that tagged form. The `(f64 bits -> text)` vector file in the
conformance evidence pins the boundaries.

**Canonical hash tuple (G14).** The hash covers the full redacted root,
including document metadata keys (`active_mode`, `active_capabilities`,
`active_profile`) because they are document values. Object keys sort
recursively by UTF-8 bytes (which preserves code-point order); secrets
are the literal `***SECRET***`; blobs are identity-only (`hash`, `size`,
`media_type`, `tainted`) with retained payloads stripped; floats follow
G13. The v2 `bound_content_hash` is length-prefixed over the fixed tuple
`(payload, schema_version, profile, resolved budget, mode, capability)`
in that order, matching `envelope::bound_content_hash`.

## 4. Lockfile format (`ACTIVE lock`)

TOML, versioned (`version = 2`; version 1 files still parse as legacy):

```toml
# ACTIVE lockfile v2 — generated by `ACTIVE lock`. Do not edit.
version = 2
toolchain = "active-cli 0.1.0 / active-core 0.1.0"
content_hash = "sha256:…"
profile = "prod"          # only when a profile is active

[main]
path = "/deploy/app.active"
sha256 = "…"

[[provenance.env]]
name = "DB_HOST"
set = true
value = "localhost"

[[provenance.env]]
name = "DB_PASSWORD"
set = true
secret = true
value_hash = "sha256:<salt-hex>:<digest-hex>"
value_salt = "<salt-hex>"

[[provenance.includes]]
path = "/deploy/base.active"
sha256 = "…"

[[provenance.include_pins]]
as = "base.active"
resolved = "base.active"
sha256 = "…"

[[blobs]]
path = "logo"
hash = "blake3:…"
size = 5
media_type = "text/plain"   # "***SECRET***" when the identity is tainted
tainted = false
```

Blob records are emitted for every identity in the evaluated tree,
sorted by path (deterministic). A tainted identity's `media_type` is
redacted on the same rule as every other export path (spec/blob.md §7).

- `ACTIVE lock <file> [-o out.lock]` (default stdout):
  parses, evaluates, collects provenance, prints. Parse/eval
  failures and unreadable includes exit non-zero. TOML Null
  warnings from evaluation surface on stderr; the lock still
  emits (evaluation succeeded).
- Lockframe: a file output records `main.path` and `includes[].path`
  relative to the output file's directory (with `/` separators), so
  verification resolves them from the lock's own location and a moved
  project keeps its lock valid. A stdout lock records absolute paths.
  `include_pins[].resolved` is independent: it is always relative to the
  root document's directory.
- Replay semantics: re-running `ACTIVE lock` on the same inputs
  reproduces the hash byte-for-byte; any drift (changed env
  value, edited include, new toolchain) changes the hash or the
  manifest. An audit produced under custom resource limits replays
  only under the same limits: `active debug --replay` accepts the
  same `--max-*` flags as `eval`. The wall clock is disabled for
  the canonical hashing pass, so a successful export cannot be
  followed by a timing-dependent audit failure.
  Comparison tooling is future work (not C-051).

### 4a. Eval-time include resolution (L-P03)

A lockfile is also an allow-list for the include graph. Two opt-in
sources pin include content:

- **Inline pins** live in the importing document:
  `include "lib.active" sha256:<hex>` (spec/grammar.md §2.7). The digest
  is SHA-256 over the included file's bytes; a mismatch is E041 before
  the file parses. The document is self-describing: no sidecar is needed
  to detect substitution.
- **Lockfile pins** live in `provenance.include_pins`, each carrying
  `as` (the spelling), `resolved` (root-relative identity), and
  `sha256`. A lockfile written before `include_pins` existed resolves
  through `provenance.includes` instead: those entries live in the
  lockfile frame and contribute content hashes for warn-mode admission,
  never identity matches.

Resolution order and outcomes, per include:

1. If the document carries an inline pin, it must match (E041). A match
   records source `inline`.
2. If a lockfile is active, the include's canonical path is matched
   first, then its spelling. The pinned digest must equal the actual
   digest (source `lock-path`). Matching the resolved path first keeps
   one relative spelling used from two directories unambiguous.
3. Under enforcement - an explicit `--lockfile` or a trusted
   evaluation's auto-discovered sibling `active-lock.toml` - a lockfile
   that carries identity pins is an allow-list of edges: an include
   absent from it is E041 even when its bytes equal another pinned
   file's. `--resilient` never relaxes this.
4. A lockfile without identity pins (legacy v1) cannot name the includes
   it did not see. A recorded include whose bytes changed is refused
   before parse as `lockfile drift`. A new edge resolves by content in
   warn mode: a digest match is source `lock-content`, and a miss is
   W017 unless the include already carries a matching inline pin.
   Strict evaluation still exits nonzero on the warning; `--resilient`
   continues.
5. A trusted evaluation with no sibling lockfile is unchanged: includes
   are verified by signature (when trusted) and no lock warning is
   emitted. The lock is used by default only when one exists.
6. Both pin sources apply when both are present; neither disables the
   other.

`active eval --audit-trail` records every resolution as
`include_resolutions = [{ path, resolved, sha256, source }]` where source
is one of `inline`, `lock-path`, `lock-content`, `unpinned`. The receipt
therefore shows what was verified, not merely what was read.

### 4b. Content-addressed include store (L-P04)

The store is an optional local cache keyed by include content:

- Root: `$ACTIVE_STORE_DIR`, else `~/.cache/active/store` (`%USERPROFILE%`
  on Windows; `~` is not shell-expanded by the CLI). These defaults apply
  to the `active store` subcommands; `eval` and `lock` require an
  explicit `--store <dir>`. An empty variable is ignored (it never
  falls back to the cwd); an explicitly empty root is refused.
- Layout: `<root>/<sha256-hex>` holds one immutable file with the source
  bytes read. The root is created `0700` on Unix; entries are written
  through a private `0600` temp file, fsynced, made `0400`, then renamed
  into place. Windows uses inherited ACLs plus the read-only attribute.
  The root is created or re-restricted `0700` on Unix; a root that is a
  symlink or a non-directory is refused. Entry names that are symlinks,
  hard links, or special files are never listed, read, or chmodded.
  An existing entry is never rewritten unless its content no longer
  matches its name, in which case the damaged entry is repaired.
- `active eval --store <dir>` and `active lock --store <dir>` publish
  every include's source bytes (the same bytes that were verified). A
  store failure is W018, an informational diagnostic: evaluation
  continues with the bytes already read, the value tree and content
  hashes are unchanged, and no exit status changes.
- `active store add <file> [--store <dir>]` publishes one file and
  prints its digest. `active store list [--json]` lists complete
  (lowercase, regular, non-symlink) entry names.
- `active store gc --root <lockfile>... [--dry-run]` deletes every entry
  not named by a `sha256` in the given lockfiles (main, includes, and
  identity pins alike), using the canonical lock parser. A malformed,
  truncated, or hashless lockfile fails closed. `--all` is the explicit
  full wipe and may not be combined with roots. `gc` is not safe
  concurrently with `put`: run it between evaluations.
- The store never feeds evaluation on its own: includes still resolve
  from their declared paths and are verified against their pins. It
  deduplicates repeated includes of identical bytes at the storage layer
  and gives a lockfile's hashes a local address; it does not change
  determinism, content hashes, or budgets.

### 4c. Include interning (L-P32)

Within one parse invocation, the include loader interns its inputs by
content:

- The source bytes of a canonical filesystem include path are read once
  and reused for every later include of that path in the same parse, so
  the parse does not observe a mid-parse rewrite of a path that still
  resolves. Path resolution still happens per include: a path deleted
  mid-parse fails E017, and a file that grew past a byte bound can still
  fail the metadata pre-check (E028/E029). A host include loader is still
  called once per resolution; its bytes are interned by hash, never by
  path.
- Lexed token streams are keyed by the SHA-256 of those bytes. A second
  include with identical content re-parses the cached stream instead of
  lexing again. The cache is per parse invocation and never persisted.
- Cache admission is bounded: a stream over 65,536 tokens is not cached,
  and the cache retains at most 256 streams / 200,000 tokens. A skipped
  stream is simply lexed again; correctness is unaffected.
- Interning is not a verification shortcut: every include still resolves
  its path, verifies signatures and pins against the bytes read, and
  records its own resolution. A cached stream is used only after those
  checks pass, and the once-per-include wall-clock check still runs on a
  cache hit.
- Interning does **not** bound a malicious fan-out of distinct files.
  Every resolution still charges the C-002 resolution and byte budgets,
  so a graph of many distinct files fails at the same cap with or without
  the cache.
- Values, diagnostics, content hashes, and audit fields are unchanged;
  file stamps and annotations stay per include (the cached stream carries
  no file identity).

### 4d. Content-keyed evaluation cache (L-P29)

`active eval --cache <dir>` skips the evaluation walk when the same
inputs were evaluated before.

- **Key.** A keyed BLAKE3 MAC over a canonical object holding the
  evaluator's version; the root source's
  SHA-256; every include's resolved identity and content SHA-256 (sorted);
  the mode; the statically selected profile name (a computed
  `active_profile` is covered by the source and environment values in
  the key); the resolved budget (all seven
  fields); the values of every environment variable the document names
  through `env.NAME` or `secret.env(NAME)` (a secret locator's value
  seeds the redactor, so it changes redacted output); the input file's
  path as invoked (diagnostics name it); and a variant string naming every
  non-default output selector (`--path`, `--materialise-blobs`,
  `--reader-version`, `--resilient`, and the sandbox/capability name).
  The output format is not in the key: one entry stores JSON, YAML, and
  TOML. A module with no source bytes has no key and is never cached.
- **MAC key.** The MAC key is 32 random bytes in a per-user file outside
  the cache directory: `$ACTIVE_CACHE_KEY_FILE` when set, else
  `%APPDATA%\active\cache-key` on Windows, or
  `$XDG_CONFIG_HOME/active/cache-key` / `~/.config/active/cache-key` on
  Unix. It is created owner-only (`0700` directory via an atomic
  directory mode, `0600` file) and is
  read through a small size bound; a non-regular file, one readable by
  another user, or one owned by another user is refused on Unix
  (Windows ACLs are not checked). The key file's owner is the identity
  the cache root and every served entry must also have. Without a usable
  key the run proceeds
  uncached. The key makes the entry names unguessable from a directory
  listing: another user cannot compute the name for chosen inputs,
  confirm a guessed environment value, or forge an entry. A shared cache
  would need the same key on every machine, so entries are not portable
  across users or machines.
- **Hit.** The stored diagnostics are reported and the stored rendering
  is printed, so stdout, stderr, and the exit status match the run that
  filled the entry. There is no evaluation walk and no budget clock.
- **Miss.** The run evaluates normally, prints the requested format, and
  publishes one entry atomically (temporary file then rename; root
  `0700`, entry `0600` on Unix). A write failure is a warning;
  evaluation output and exit status are unchanged. The rendered parts,
  the serialized diagnostics and export diagnostics, the errors, and the
  receipt are checked against the 32 MiB entry cap before they are
  cloned into an entry; a larger output is printed but not cached.
- **Entry.** `{version, key, json, yaml, yaml_error, toml, toml_error,
  export_diagnostics, diagnostics, receipt}`. The `receipt` uses the
  L-T06 v2 shape: it binds the served value's content hash to the schema
  version, profile, resolved budget, mode, and capability, and names the
  capability field. A resilient
  run that did not pass is stored with `outcome = "failed"` and
  `failure_class = "other"` (the envelope path is what names a stage).
  A run whose profile or schema selector is material writes no entry at
  all, so the selector never reaches disk in cleartext or as a hash.
  `version` and `key` are re-checked on read; a mismatch is a miss. Only
  a regular file readable by its owner is read, through a bounded
  reader, and the opened handle is re-checked, so a symlink, FIFO,
  device, or shared file at an entry name is
  a miss instead of a block or an unbounded read. An entry that is
  missing its JSON rendering or one outcome for YAML/TOML is incomplete
  and is a miss with a warning, never empty output. A malformed or
  oversized entry is a miss with a
  warning. The entry holds the printed output, including materialised
  blob payloads, so the directory needs the source's confidentiality.
- **Trust boundary.** The cache directory is trusted local state. On
  Unix a pre-existing root that is a symlink, is not a directory, is
  group/other-writable, or is owned by anyone other than the key file's
  user is
  refused, as is an entry readable by or owned by another user; Windows
  ACLs are not
  checked. The
  MAC key stops another user who can list or write a private directory
  from forging an entry for chosen inputs; an account that can read the
  key file can do anything its owner can. The receipt records provenance;
  it is not a signature over the entry.
- **Run observers and admission.** `--audit-trail`, `--envelope`, and
  `--trace` record or observe the run itself (peer identity, signature,
  event trail), and `--store`, `--trust-keys`, `--lockfile`, and
  `--envelope-trust` add side effects, signature policy, or parse-time
  diagnostics that are not in the key. Each of those bypasses the cache
  and says so on stderr, including when parsing then fails.
- **No input substitution.** The cache never supplies an input: includes
  are re-read and re-verified on every run, and the cache lookup happens
  after parse admission. Changed content changes the key, so an entry
  cannot serve a stale result. Entries from a rebuild at the same
  declared version can differ in evaluator semantics; delete the
  directory to clear them.
- **Conformance.** The cache is a CLI optimization, not a language
  surface: the FFI and WASM embeddings expose no cache entry point, and
  `active-core` exports only the key/entry helpers, so it carries CLI
  tests rather than a cross-embedding conformance vector.

### 4e. Plan/apply receipts (L-P16)

`active plan old new` is the review artifact for a replacement;
`active apply` is the audited execution.

- **Confinement.** Plan paths are stored relative to `--root` (default:
  the inputs' common ancestor). `apply` resolves them under its own
  `--root` (default: the plan file's directory), rejects absolute paths
  and `..`, canonicalizes the target, and refuses anything outside the
  root or any non-regular file.
- **Plan MAC.** `plan_hash` is a per-user keyed MAC (`mac:<hex>`, keyed
  BLAKE3 over `active.plan.v1\n` + canonical plan JSON without
  `plan_hash`), using the key file of §4d. A plan minted by another user
  or edited after approval is refused; an unkeyed plan is refused. The
  plan carries the canonical document hashes, sorted include-content
  hashes, and the evaluation context (profile, resolved budget,
  parent-escape policy, trust).
- **Apply re-verification.** Before writing, `apply` requires
  `--approve` to equal the recomputed plan MAC, then re-runs the strict
  review of the exact files under the plan's context (passing
  `--trust-keys` when the context is trusted and refusing it otherwise).
  The document hashes, include hashes, diff narrative, risk, and
  behaviour must match; any drift refuses with a stable `[drift]` class.
  The candidate is strict-parsed and evaluated, and its bytes are
  re-hashed after the final read, so an invalid or swapped document is
  never installed.
- **Replace and receipt.** The baseline is replaced atomically
  (temporary file created owner-only, then rename), preserving its mode
  and the Windows read-only bit (ACLs and extended attributes are not
  copied); the baseline is re-hashed immediately before publishing.
  The receipt temp is prepared before the baseline write, so a receipt
  path that cannot be written refuses the whole apply. A receipt may not
  overwrite the baseline, candidate, plan, an include, the sibling
  lockfile, or the trust-keys file. An existing receipt is refused
  unless `--force` is passed, and `--force` only replaces a file that is
  an apply receipt.
- **Receipt.** `{apply: {version, applied, plan_hash, root, old, new,
  steps?, operator?, receipt_hash}}`; `steps` is the applied set (the
  plan's structural steps, as approved), and `receipt_hash` is the same
  keyed MAC over the canonical receipt without `receipt_hash`, domain
  `active.plan-receipt.v1`. There is no timestamp, so the same plan and
  operator produce the same receipt bytes. The default path is
  `<plan>.receipt.json`. The receipt is published atomically (the same
  temp-and-rename discipline as the baseline).
- **Refusal receipt (L-P41).** A refused apply writes
  `{refusal: {version, applied: false, failure_class, plan_hash?,
  steps?, receipt_hash}}` to the receipt path (the same default) instead
  of a success receipt. `failure_class` is the stable `[class]` token
  (`plan-shape`, `plan-hash`, `root`, `context`, `baseline`, `candidate`,
  `drift`, `receipt`, `io`, or `other`). `plan_hash` and `steps` are
  recorded only after the plan MAC and approval verified, so untrusted
  plan text never lands in a receipt. A refusal never touches the
  baseline and leaves no temp behind; an existing receipt is still
  protected by `--force`.
- **Scope.** The plan covers one baseline file; includes are hashed and
  re-verified, but `apply` replaces only the baseline file. No shipped
  command writes multiple documents as one commit, so multi-file
  atomicity is not claimed; an overlay would need a two-phase journal
  (intent file, per-file temp, fsync, rename, recovery) and is a future
  item. Deploying other files is the caller's responsibility. The plan and receipt
  shapes are pinned by `docs/schemas/plan.json` and
  `docs/schemas/plan-receipt.json`; as CLI tooling surfaces they carry
  CLI and output-schema coverage rather than a cross-embedding vector.
- **Trust boundary.** The MAC binds a plan to the local user's key: the
  approval is meaningful only for a plan this user generated. It is not
  a signature by a third party. The receiving user should review the
  rendered diff and steps before copying the MAC into `--approve`.

## 5. Threat model notes (for C-052)

- The manifest records plain env values verbatim: authors must
  route sensitive inputs through `secret()`/`secret.env()`. A
  value that matches an evaluator secret seed, or that a profile
  places at a material path, is withheld (`secret = true`, hash
  only) even when its name is not structurally bound. Plain-env
  values already appear in evaluated output, so the manifest adds
  no new exposure class.
- Hashes (`content_hash`, `value_hash`) are integrity/drift
  signals, not encryption; SHA-256 preimage resistance is the
  operative property. `value_hash` is salted per lockfile (LANG-I-001),
  which stops precomputed tables and cross-file correlation, not an
  offline dictionary attack on a guessable secret value: a lockfile
  that carries one is as sensitive as the environment it describes.
- Lockfiles assert provenance, not trust: they record WHERE
  inputs came from, not THAT they are safe. Signature policy is
  C-052. The exception is explicit include pinning (§4a): an inline
  pin or an enforced lockfile is a content promise, and a substituted
  include fails before evaluation. A warn-mode lockfile names the
  unpinned include instead of vouching for it.

## 6. Testing contract

- Snapshot enforcement: ambient vars invisible under an explicit
  (e.g. empty) snapshot; injected vars visible.
- Manifest unit tests: plain/secret/unset vars, include
  collection, profile recording, vanishing-file hard error.
- Hash stability: same tree twice, redacted-tree equality for
  differing secrets, cross-platform determinism by construction
  (sorted keys, compact form).
- Round trip: lockfile TOML re-parses; golden fixture; `include_pins`
  render and re-parse by spelling.
- Include pinning: inline match/mismatch (E041), lock path/content
  admission, missing pin under enforce (E041) and warn (W017), both pins
  enforced together, audit receipt sources, and conformance vectors
  `0037`/`0038`.
- Store: put/get idempotence and read-only entries; GC keeps lockfile
  roots and removes the rest; dry-run touches nothing; malformed roots
  fail closed; parser publishes parsed bytes; a failing store yields
  W018 without changing the tree; repeated includes store once.
- Interning: a source is read once per parse and a lexed stream is
  reused by content hash; values, pins, resolutions, and diagnostics
  match an uninterned parse; the C-002 cap still trips on distinct
  fan-out.
- Evaluation cache: a second run is served from the entry (sentinel
  substitution proves no evaluation); source, include, profile, budget,
  mode, environment-value, and variant changes each miss; a different
  MAC key is a different identity; one entry serves JSON, YAML, and TOML;
  a corrupt, stale, or non-regular entry is a miss with a warning;
  run observers and admission flags bypass and warn; a material profile
  or schema selector writes no entry; a shared cache root or a
  world-readable entry is refused on Unix; host-built modules have no
  key; the key file round-trips, rejects malformed or oversized content,
  and sits under the platform config directory; root and entry
  permissions are owner-only on Unix; the total-cap pre-check counts
  every part; the entry's receipt binds the served value and names its
  capability.
- Plan/apply: the approval hash covers the canonical plan without
  `plan_hash`; a wrong approval, an edited plan, baseline drift, and
  candidate drift each refuse; a successful apply replaces the baseline,
  preserves its permissions, and writes a deterministic receipt whose
  hash covers the receipt without `receipt_hash`.
- CLI tests for `ACTIVE lock` incl. exit codes and `-o`.

## Sandbox closure (L-P28)

An evaluation whose inputs come from an `InputClosure` is reproducible across hosts: the closure carries the document, exact include bytes with digests, declared env names/values (or declared absence), and the host profile. The engine reads no filesystem, network, clock, or ambient environment; a read outside the closure is E048. See `spec/sandbox.md` for the closure shape, id, and per-surface checklist.

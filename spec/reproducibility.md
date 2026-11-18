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
compact JSON (`serde_json::Map` is a `BTreeMap`, so keys sort —
byte-stable), secrets already replaced by `***SECRET***`
(`value_to_json`), retained blob payloads stripped first.
Hashing the redacted, payload-free tree (never raw secrets)
keeps lockfiles publishable and makes the identity independent of
`--materialise-blobs`. The audit and replay use the same
canonicaliser, so `debug --replay --expect` reproduces an audit
`result`. Same tree → same hash on every platform (tested
cross-fixture; CI runs both).

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
- CLI tests for `ACTIVE lock` incl. exit codes and `-o`.

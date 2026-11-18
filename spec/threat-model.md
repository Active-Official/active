# Threat Model (C-052, P-051)

What ACTIVE protects, from whom, and where the boundaries are.
Read with spec/reproducibility.md (provenance, lockfiles).

## 1. Assets

1. **Secret values** (`secret()` / `secret.env()`): highest value.
   Never appear in exports, diagnostics, logs, errors, hover
   text, lockfiles, graphs, or generated docs — only the
   `***SECRET***` marker does (C-036, audited).
2. **Evaluated configuration integrity**: what deploys must be
   what was reviewed. Protected by content hashes (C-051) and,
   for overlays, signatures (this item).
3. **Provenance accuracy**: the record of where values came from
   (env names, include files, profile). Recorded verbatim for
   plain inputs; secret-bound inputs contribute hashes only.
4. **Operator control**: budgets bound every resource dimension
   (C-037); evaluation terminates or fails loudly.

## 2. Actors and trust

| Actor | Trust | Rationale |
|---|---|---|
| Document author (main file) | Trusted | Invoked the toolchain directly. |
| Include / overlay author | **Untrusted by default** | Shared libraries, vendored files, fetched snippets. Trusted only via signatures (§4) or explicit review. |
| Environment operator | Trusted for values, audited by name | Supplies env at deploy; every read is named in the manifest. Must route sensitive inputs through `secret()` — plain-env values record verbatim. |
| Schema author | Trusted for types, not for data | Schemas constrain but never inject values. |
| AI agents (generator/editor) | Constrained | Prompt pack (C-053) + read-only queries (graph, hover); writes go through validated parses. |

## 3. Attack surfaces and mitigations

### 3.1 Malicious or compromised includes

Includes execute as data: they contribute bindings, tables, and
patches to the merged module. A hostile include can exfiltrate
(via interpolation into exported strings — blocked for secrets
by S004, but plain values DO export), override keys (last-wins
merge), or inject patches (applied post-evaluation).

Mitigations:

- Literal relative paths only; parent escapes and absolute paths
  rejected (E015); cycles rejected (E016); depth ≤ 32.
- Private includes never leak bindings (C-025).
- **Signed includes (§4)**: with `--trust-keys`, every include
  must carry a valid signature from the trusted set (E032
  otherwise). Unsigned ecosystems stay working by default;
  sealed deployments pass the flag.
- **Content-pinned includes (L-P03/L-P06)**: an inline
  `include "x" sha256:<hex>` binds the bytes the document expects, and
  an enforced lockfile (`--lockfile`, or the sibling `active-lock.toml`
  under `--trust-keys`) pins every include by resolved path and spelling.
  A changed include is E041; an include absent from an enforced lock is
  E041. A legacy lockfile without identity pins refuses a changed
  recorded include as `lockfile drift`, and resolves a new edge by
  content with W017 on a miss unless the include carries a matching
  inline pin.
- **Residual risk (documented)**: lockfiles are not signed. An attacker
  who can ship the document *and* its sibling lockfile can pin their own
  bytes; enforcement proves the evaluated graph matches the lock, not
  that the lock is trustworthy. Pin the lockfile's own hash through the
  deployment channel, or sign the document tree, when that matters.
- **Include store (L-P04, opt-in)**: `--store` writes the raw bytes of
  every include, including any secrets they contain, into a local cache
  (default `~/.cache/active/store`; root `0700`/entries `0400` on Unix,
  inherited ACLs plus the read-only attribute on Windows). The cache is
  read-only after write but not encrypted, and removing entries requires
  an explicit `active store gc`. Point `--store` at a directory with the
  same confidentiality as the sources; an in-repository path can be
  committed by `git add`, so keep it out of the worktree or ignore it.
- Patches overlapping `@ai_immutable` fail (E019/C-034).

### 3.2 Secret exfiltration

- Taint-as-kind: secrets propagate through every construction;
  exports redact; interpolation of secrets is a hard error
  (S004), not a tainting (stronger).
- Manifests, lockfiles, diagnostics, hover, hints, docs, graphs,
  and import errors carry markers/hashes, never values (each
  audited by test: search the artifact for the planted value).
- `value_hash` on a secret-bound env read is salted per lockfile, with
  the salt stored beside it (LANG-I-001). The salt defeats precomputed
  tables and cross-lockfile correlation; it cannot stop an offline
  dictionary guess at a low-entropy value, so a lockfile that carries a
  `value_hash` is itself sensitive material.
- Residual risk (accepted, documented): an author who puts a
  secret in a PLAIN `env` read or string literal has routed
  around the protection — the toolchain cannot distinguish
  sensitive plain data. Mitigation is social: lint rule L004
  flags probable literals, and this model says so plainly.
- Incremental memo entries (C-055): the memo is same-process
  memory holding evaluated values, including plaintext
  intermediates — same exposure as the evaluator's own value
  map, and never a cross-process artifact. Three properties
  bound the risk, all pinned by test or construction: (1)
  fingerprint keys are salted per memo, so a key is not a
  stable cross-process oracle for short secret-derived values;
  (2) `IncrementalMemo`'s `Debug` reports shape only (entry
  count, telemetry), never document content; (3) the 64-bit
  FNV-1a width is load-bearing — a collision would serve a
  stale value. Forging one against a salted, same-process memo
  (e.g. via a crafted document in a shared language-server
  engine) requires per-memo 2^64-scale work and yields only
  that process's values: documented here, not defended
  further — verify-on-hit would cost the memo its purpose.

### 3.3 Resource exhaustion

Budgets (nodes, depth, wall-clock, array length, total keys,
value size) fail closed (X001–X006) in both modes. Fuzzing
(C-012) proves arbitrary bytes terminate without panic.

### 3.4 Supply-chain: toolchain and schemas

- Lockfiles bind content hash + provenance + toolchain version;
  drift changes the hash.
- `ACTIVE generate` output is data-derived; schema authors cannot
  inject values (defaults render; dynamic bounds skipped).
- Crates.io provenance and signed release binaries are C-056.

### 3.5 Signature scope

`--trust-keys` verifies the root document and its includes against the
trust set (see LANG-C-007). Signatures bind the signed identity
(relative path, optional role and version) and may carry an expiry; the
trust file may revoke key ids (LANG-I-002).

- Signatures authenticate AUTHORSHIP/INTEGRITY of the bound bytes for the
  bound identity, not safety of content: a signed include is still data,
  still validated, still budgeted. Trust != sandbox escape.
- Identity binding: a sidecar records the identity `path`, and
  verification refuses any other path (E039). Copying a signed overlay
  elsewhere fails closed.
- The signed identity is the spelling the importing document uses, so a
  signed `lib.active` copied into another directory under the same
  spelling is indistinguishable to the signature check. Eval-time lock
  pins key on the root-relative resolved identity for exactly this reason
  (L-P03, spec/reproducibility.md §4a): enable an enforced lockfile when
  same-spelling includes from several directories are in scope.
- Expiry: a sidecar may carry `expires = "YYYY-MM-DD"`; past it the
  sidecar fails closed (E037). Beyond that there is no timestamping: a
  still-valid, still-bound sidecar that is replayed verifies, and
  freshness comes from version control and lockfiles.
- Revocation: `revoked = ["kid", ...]` at the top of the trusted-keys
  file; a sidecar naming a revoked kid fails closed (E038). Rotation adds
  the new public key, re-signs, then removes the old.
- Key files are bearer credentials: `0600`, never committed,
  never logged. The CLI reads secret keys only from files (never
  argv, which leaks via process lists).

## 4. Signed overlays: format and enforcement

- **Algorithm**: Ed25519 over `sha256(identity || 0x00 || bytes)`, where
  the identity is the normalized `path`, optional `role` and `version`,
  and the optional `expires` (SHA-512 internal; deterministic, no
  randomness at verify time). A signature verifies only for its bound
  identity (LANG-I-002).
- **Keys**: TOML trusted-keys file —
  `[[keys]] id = "release-2026" public = "<64-hex>"`, plus an optional
  top-level `revoked = ["kid", ...]` list of revoked ids.
  Secret keys live in `<name>.secret` files (64 hex chars =
  32-byte seed); `ACTIVE keygen --out <prefix>` creates
  `<prefix>.secret` + `<prefix>.public` (the latter paste-ready
  for the TOML).
- **Sidecar**: `<file>.sig` with `kid = "<id>"`, the bound `path`
  (required), optional `role`/`version`, optional
  `expires = "YYYY-MM-DD"`, and `sig = "<128-hex>"` (TOML). Detached:
  the signed file is never modified, so signatures survive formatters.
  The identity the signer records is the exact spelling the document
  will use (`active sign --as <path>`); the root document's identity is
  the path the receiver passes to `parse_file_with_trust`.
- **No algorithm field** (LANG-I-009): the sidecar format carries `kid`,
  the identity fields, `expires`, and `sig`; the verifier's algorithm is
  Ed25519 and an unknown field cannot change it. A sidecar that claims a
  different algorithm is a no-op claim; a future algorithm needs a
  format-version marker before it can be negotiated.
- **Enforcement**: `Parser.trust: Option<TrustedKeys>`
  (`None` = no enforcement, all legacy behavior preserved;
  `parse_file_with_trust` feeds it). When set, every `include`
  load reads once and verifies the bytes in memory against a
  sibling sidecar from the set — no check/use gap on content;
  anything else is E032 (unsigned sidecar, unknown key,
  signature mismatch — each names the file and the reason, never
  key material). Key ids obey a strict charset at every layer
  (written, parsed, echoed) so diagnostics stay log-safe. CLI:
  `--trust-keys <file>` on
  eval/check/convert/lock/graph/doc/generate plus impact/why
  (all commands that resolve includes);
  `ACTIVE sign <file> --key <secret> --kid <id>` writes the
  sidecar (invalid kids refused before any file is touched);
  `ACTIVE verify <file> --trust-keys <toml>` is point-in-time
  attestation (exit 0/1).
- **Carve-outs (documented, not gaps)**: formatting never loads
  includes (`resolve_includes: false`), so `ACTIVE fmt` needs no
  flag; lint loads but never evaluates (read-only analysis).
  The language server evaluates dirty buffers for hover values
  without trust plumbing — hermetic and budgeted, but unsealed:
  do not open untrusted workspaces with secrets in env.

## 5. Key-management expectations

1. Generate per-purpose keys (release, team, automation);
   never reuse a personal signing key for releases.
2. Store secret keys in a vault or `0600` files outside the repo
   (`ACTIVE keygen` creates the secret half `0600` atomically, with no
   write-then-chmod window, and pins the public half `0644`; on Windows
   both inherit the directory ACL, so generate them in an ACL-restricted
   directory);
   public halves live in-repo beside the documents they guard.
3. Rotate by adding the new public key, re-signing, then
   removing the old — the window where both verify is the
   migration window (revocation list in the trusted-keys file; see §3.5).
4. Signatures provide authorship for the bound path/role/version until
   expiry; lockfiles and version control provide freshness. Use both.
5. Compromise response: rotate keys, re-sign known-good bytes
   (verified against lockfile hashes from before the incident),
   redeploy. The lockfile hash tells you whether bytes changed.

## 6. Out of scope (C-070 re-affirmed)

No Turing-complete policy engine, no networked transparency log,
no encrypted values in documents (redaction, not encryption, is
the mechanism), no sandboxing beyond hermetic evaluation.

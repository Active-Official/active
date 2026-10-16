# Threat Model (C-052, P-051)

What SOLID protects, from whom, and where the boundaries are.
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
- Patches overlapping `@ai_immutable` fail (E019/C-034).

### 3.2 Secret exfiltration

- Taint-as-kind: secrets propagate through every construction;
  exports redact; interpolation of secrets is a hard error
  (S004), not a tainting (stronger).
- Manifests, lockfiles, diagnostics, hover, hints, docs, graphs,
  and import errors carry markers/hashes, never values (each
  audited by test: search the artifact for the planted value).
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
- `solid generate` output is data-derived; schema authors cannot
  inject values (defaults render; dynamic bounds skipped).
- Crates.io provenance and signed release binaries are C-056.

### 3.5 Signature scope limits (explicit non-goals)

- Signatures authenticate AUTHORSHIP/INTEGRITY of overlay bytes,
  not safety of content: a signed include is still data, still
  validated, still budgeted. Trust ≠ sandbox escape.
- No revocation protocol in v1: rotate by shipping a new
  trusted-keys file (old signatures fail closed against the new
  set). Documented in key management (§5).
- No timestamping: replay of a once-valid signed file is
  possible; freshness comes from version control + lockfile
  hashes, not from signatures.
- Key files are bearer credentials: `0600`, never committed,
  never logged. The CLI reads secret keys only from files (never
  argv, which leaks via process lists).

## 4. Signed overlays: format and enforcement

- **Algorithm**: Ed25519 over raw file bytes (SHA-512 internal;
  deterministic, no randomness at verify time).
- **Keys**: TOML trusted-keys file —
  `[[keys]] id = "release-2026" public = "<64-hex>"`.
  Secret keys live in `<name>.secret` files (64 hex chars =
  32-byte seed); `solid keygen --out <prefix>` creates
  `<prefix>.secret` + `<prefix>.public` (the latter paste-ready
  for the TOML).
- **Sidecar**: `<file>.sig` with `kid = "<id>"` and
  `sig = "<128-hex>"` (TOML). Detached: the signed file is never
  modified, so signatures survive formatters.
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
  `solid sign <file> --key <secret> --kid <id>` writes the
  sidecar (invalid kids refused before any file is touched);
  `solid verify <file> --trust-keys <toml>` is point-in-time
  attestation (exit 0/1).
- **Carve-outs (documented, not gaps)**: formatting never loads
  includes (`resolve_includes: false`), so `solid fmt` needs no
  flag; lint loads but never evaluates (read-only analysis).
  The language server evaluates dirty buffers for hover values
  without trust plumbing — hermetic and budgeted, but unsealed:
  do not open untrusted workspaces with secrets in env.

## 5. Key-management expectations

1. Generate per-purpose keys (release, team, automation);
   never reuse a personal signing key for releases.
2. Store secret keys in a vault or `0600` files outside the repo
   (`solid keygen` sets `0600` on Unix; on Windows use an
   ACL-restricted directory);
   public halves live in-repo beside the documents they guard.
3. Rotate by adding the new public key, re-signing, then
   removing the old — the window where both verify is the
   migration window (no revocation list in v1).
4. Lockfiles and version control provide freshness; signatures
   provide authorship. Use both.
5. Compromise response: rotate keys, re-sign known-good bytes
   (verified against lockfile hashes from before the incident),
   redeploy. The lockfile hash tells you whether bytes changed.

## 6. Out of scope (C-070 re-affirmed)

No Turing-complete policy engine, no networked transparency log,
no encrypted values in documents (redaction, not encryption, is
the mechanism), no sandboxing beyond hermetic evaluation.

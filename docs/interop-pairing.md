# Interop pairing record

> Single source of truth for which standalone-crate versions were verified
> against this workspace (`Active-Official/active`). `VERSIONING.md` records
> the policy; this file records the pairing. `active-core/tests/docs_contract.rs`
> enforces the table, the pin, and the re-verification rule: a version bump
> without an updated record fails the suite.

## Verified triple

**Verified 2026-11-14; registry facts re-checked 2026-11-16.**

| Component | Version | Registry | Canonical home |
|---|---|---|---|
| `active-lang` (this workspace: `active-core`, CLI, LSP, FFI, WASM) | 1.4.0 | crates.io (frozen line — see `VERSIONING.md` §2) | <https://github.com/Active-Official/active> |
| `active-infinity` (binary, zero-copy codec) | 2.4.2 (published 2026-11-03) | <https://crates.io/crates/active-infinity> | <https://github.com/Active-Official/active-infinity> |
| `active-rpc` (transport) | 2.2.0 (published 2026-10-31) | <https://crates.io/crates/active-rpc> | <https://github.com/Active-Official/active-rpc> |

**Rule:** any `active-core` minor bump re-verifies the bridge against the
LANG-E-002 conformance vectors in `conformance/interop/` before the bump is
released. The pairings are ranges in the standalone repositories' manifests
(owned there); this record is what tells a release operator which concrete
versions were actually verified together.

## Owned vectors and the executable checker (LANG-E-002)

The text → binary → text contract is owned here, in `conformance/interop/`:

- `cases/*.active` — documents the published codec accepts (scalars,
  containers, durations, quantities, bytes, secrets, multiline text).
- `structural/*.active` — expression graphs; the codec preserves them, and
  its JSON projection renders expression nodes as markers.
- `current/*.active` — blobs, tainted blobs, profiles, and patches. These
  need an `active-core` newer than the published 1.4.0 that the pinned
  codec depends on, so they carry a text-side expectation only until the
  next re-pair.
- `expected/`, `expected-structural/`, `expected-current/` — the exact
  redacted JSON this workspace exports, pinned by
  `active-core/tests/interop_vectors.rs`.
- `expected-infinity/` — the codec's redacted projection for every paired
  case, pinned by the checker.
- `checker/` — a standalone crate (outside the Active workspace) that pins
  `active-infinity =2.4.2` and runs every paired case through the real
  codec: `from_active_source` → `encode` → `validate` → `decode` →
  re-`encode`, asserting structural equality, byte-identical canonical
  output, a stable content hash, a fixed-point projection, and semantic
  equality with this workspace's export after unit normalization.

Run it from the repository root:

```sh
cargo run --locked --manifest-path conformance/interop/checker/Cargo.toml
```

CI runs the same command in the Conformance workflow's `interop` job, so a
codec release that changes a value kind, a unit, a redaction path, or the
canonical form fails here before a pairing claim is made.

## Ownership of the binary round-trip

The **codec** (`active-infinity`) and its wire format live in
`Active-Official/active-infinity`; this workspace owns the **text side**
and the vectors above. No binary-encoding code exists here: the C ABI
(`active-ffi`) and the WASM surface (`active-wasm`) carry text and the
stable JSON value envelope.

**Re-verification procedure** (owned by the release operator of whichever
side moves):

1. Build the Infinity codec at the version in the table.
2. Run `cargo run --locked --manifest-path conformance/interop/checker/Cargo.toml`.
   Every paired case must report `codec ok, semantics ok`.
3. Copy any new `current/` case into `cases/` once the codec's core
   dependency parses it, regenerate both expectations, and record the new
   versions and date in the table.
4. If a paired case fails, open an issue on the side that broke the
   contract before releasing either side.

A cross-repo CI job that runs step 2 in the Infinity repository remains an
Infinity-owned follow-up: this workspace ships the vectors, the expectations,
and a checker that already executes the published crate, so the Infinity
repository can adopt the same command without owning any vector content.

## Pairing status and the next re-pair

The published `active-core` 1.4.0 is older than this workspace's source:
blob literals, profile blocks, and `@patch` directives parse here but not
in the published crate the codec depends on. Infinity 2.4.2 also does not
compile against this workspace's `active-core` API today (its bridge
expects the published enum set). Both sides therefore keep the paired tier
on the published subset; the `current/` tier is the executable contract
the next core/Infinity re-pair must satisfy. `active-core/tests/docs_contract.rs`
keeps the recorded triple, the checker pin, and the shipped version in
lockstep so this drift cannot be released silently.

## Why this record exists (LANG-E-003)

The three repositories are coupled by semver ranges, not pins (`^1.4.0`,
`^2.1.0`). Without a record, nothing says which standalone versions were
verified against this workspace's text behaviour, and a range bump on either
side can drift silently. The table, the enforced pin, and the re-verification
rule are the guard.

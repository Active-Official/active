# Interop pairing record

> Single source of truth for which standalone-crate versions were verified
> against this workspace (`Active-Official/active`). `VERSIONING.md` records
> the policy; this file records the pairing.

## Verified triple

**Verified 2026-11-14; registry facts re-checked 2026-11-16.**

| Component | Version | Registry | Canonical home |
|---|---|---|---|
| `active-lang` (this workspace: `active-core`, CLI, LSP, FFI, WASM) | 1.4.0 | crates.io (frozen line — see `VERSIONING.md` §2) | <https://github.com/Active-Official/active> |
| `active-infinity` (binary, zero-copy codec) | 2.4.2 (published 2026-11-03) | <https://crates.io/crates/active-infinity> | <https://github.com/Active-Official/active-infinity> |
| `active-rpc` (transport) | 2.2.0 (published 2026-10-31) | <https://crates.io/crates/active-rpc> | <https://github.com/Active-Official/active-rpc> |

**Rule:** any `active-core` minor bump re-verifies the bridge against the
LANG-E-002 conformance vectors before the bump is released. The pairings are
ranges in the standalone repositories' manifests (owned there); this record
is what tells a release operator which concrete versions were actually
verified together.

## Ownership of the binary round-trip (LANG-E-002)

The text → binary → text round-trip is owned **entirely** by
`Active-Official/active-infinity`:

- The codec, its wire format, and its own round-trip test suite live in that
  repository (crate `active-infinity`, 2.x line).
- This workspace owns the *text* side only: the language, `active-core`, the
  tooling, and the conformance harness that pins text → value behaviour
  (`conformance/`, `spec/conformance.md`).
- No binary-encoding code exists in this repository, and none is planned
  here: the C ABI (`active-ffi`) and the WASM surface (`active-wasm`) carry
  text and the stable JSON value envelope, not the binary wire format.

**Re-verification procedure** (owned by the release operator of whichever
side moves):

1. Build the Infinity codec at the version in the table.
2. Encode and decode every document under `conformance/cases/` plus the
   `examples/` tree through the codec, comparing against this repo's
   `conformance/expected/` value trees.
3. Record the date and both versions in this table, or open an issue on the
   side that broke the round-trip.

A cross-repo CI job that runs step 2 automatically is tracked by the Infinity
repository (it owns the codec and its CI); this workspace provides the
vectors and the expected value trees. Until that job exists, the pairing is a
manual release step recorded here.

## Why this record exists (LANG-E-003)

The three repositories are coupled by semver ranges, not pins (`^1.4.0`,
`^2.1.0`). Without a record, nothing says which standalone versions were
verified against this workspace's text behaviour, and a range bump on either
side can drift silently. The table above, plus the re-verification rule, is
the guard.

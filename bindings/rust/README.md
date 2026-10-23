# External Rust Usage (Tier-1)

Non-monorepo Rust users depend on the published `active` facade crate
— no workspace checkout needed. The full contract lives in
[`active/README.md`](../../active/README.md) (install, usage,
non-monorepo example, MSRV). The contract map:

1. **Opaque handles** — N/A at this layer (safe Rust API, no raw
   pointers); raw-ABI users go through `active-ffi` with opaque
   handles per `docs/FFI.md`.
2. **Allocator Model A** — Rust ownership throughout; raw-ABI users
   free only through `active_*_free`.
3. **ABI gate** — the facade re-exports the same `active_core`
   (Cargo guarantees identity); raw-ABI users hit the major gate.
4. **Harness parity** — `cargo test -p active --doc` green (the
   doctest parses and evaluates externally).

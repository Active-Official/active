# External Safe Rust Crate (CHECKLIST 1.009, PROPOSAL section 2.1 item 8)

**Decision (DERIVED, owner authority):** no new crate. The published `active`
facade (crates.io, unified track, currently 1.4.0) already re-exports the
full `active_core` public API, so non-monorepo Rust users depend on `active`
and never touch the workspace. A second wrapper would add an import layer
with zero new safety. The crate-level doctest on `active/src/lib.rs` is the
standing proof that the external path parses and evaluates.

External users who need the raw C ABI instead use the published
`active-ffi` crate directly (opaque handles, Allocator Model A, ABI gate),
documented in `docs/FFI.md`.

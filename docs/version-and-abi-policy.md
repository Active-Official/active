# Version alignment and ABI policy (CHECKLIST D.005, PROPOSAL §4.2)

One workspace version, one frozen ABI, explicit per-language packaging.
This document is the compatibility contract agents and operators read
before depending on anything Active ships.

## 1. Single-sourced workspace version

The Rust workspace releases as one version (`[workspace.package]
version` in the root `Cargo.toml`; every member crate uses
`version.workspace = true`). Current line: **1.4.0**. Tags are
`v<version>` on main and drive `release.yml`. MSRV 1.85 advances
only with a minor/major bump and a checklist note.

`active-infinity` is the single exception: an independent `1.0.0`
name-reservation track that converges to the workspace version when
its real surface lands (enforced by `packaging.rs`).

## 2. Tier-1 package versions (all track the 1.4.0 line)

| Language | Distribution | Version | Install |
|---|---|---|---|
| Rust (external crate) | `active` on crates.io | 1.4.0 (workspace) | `cargo add active` |
| Python | `active-lang` on PyPI (name verified free; unscoped `active` taken) | 1.4.0 | `pip install active-lang` |
| JS/TS | `active-lang` on npm (unscoped `active` taken; `@active-lang/active` reserved as fallback) | 1.4.0 | `npm install active-lang` |
| Go | `github.com/Active-Official/active-go` (external repo, library) | tracks ABI v1 + release wasm (harness pin `f5bd6e2`) | `go get github.com/Active-Official/active-go@latest` |

Language packages publish from this tree and are live at the 1.4.0
line (verified 2026-10-22): npm `active-lang`, PyPI `active-lang`, and
crates.io `active-core` all serve 1.4.0. The PyPI path publishes through
OIDC trusted publishing (`pypi.yml`, no stored token); npm and crates.io
use their registry tokens in the release flow. Registry publication is
still an explicit, human-approved release act: a normal commit never
publishes.

## 3. Frozen C ABI v1 (docs/FFI.md, never weakened)

Opaque handles, Allocator Model A (core allocates, host frees only
through `active_*_free`), ABI version gate at load, panics and null
handles as language-level errors. Every Tier-1 binding enforces the
gate at initialization; a major mismatch is a hard error:

| Binding | Gate location | Evidence |
|---|---|---|
| Python | `Module()` reads `active_abi_version`, raises `ActiveError` unless major is 1 | `bindings/python/active.py`, `test_active.py` ABI test |
| JS/TS | `load()` rejects stale/foreign modules | `bindings/js/active.js`, `test_active.js` |
| Go | `abiMajor = 1` check in `active.go` | external `active_test.go` + harness `go-native` |
| Rust facade | same-version `active_core` re-export (Cargo guarantees identity); raw ABI via `active-ffi` for advanced use | `cargo test -p active --doc` |

## 4. Compatibility guarantees

- Diagnostic codes are stable API: existing codes never change
  meaning; new codes are additive within the 1.x line.
- Evaluation semantics of shipped constructs never change
  incompatibly; new schema features are additive.
- WASM modules, cdylibs, and bindings interoperate across patch and
  minor releases as long as ABI major 1 holds; the gate fails
  closed otherwise.
- A patch release ships correctness fixes only; a minor release
  bundles additive surface with conformance-plus-release notes;
  majors only for breaking changes, which the 1.x line avoids.

# Go Binding Specification (DERIVED adaptation, CHECKLIST 1.001-1.003)

**Decision (owner authority, recorded 2026-10-20):** the canonical Go binding
lives in its own repository, `Active-Official/active-go`, rather than as an
in-repo `bindings/go/` duplicate. Rationale: the binding embeds the WASM
module via wazero with zero cgo, so it versions against the release artifact,
not the Rust workspace; a second in-repo implementation would split the
oracle and double the maintenance tail for zero user benefit.

## Assessed state (cloned and read 2026-10-20)

- Opaque `Module` handle confined to its goroutine; explicit close discipline.
- `abiMajor = 1` gate at load; major mismatch fails closed.
- `Result` envelope mirrors the WASM JSON output; secret redaction inherited
  from the module (no separate redaction code to drift).
- Own conformance test parses the shared `conformance/manifest.toml` (cases,
  pins) and asserts identical diagnostic codes; see `active_test.go`.
- Remote CI runs the manifest on every change (repo live, updated 2026-10-19).

## In-repo coverage

- `active-wasm/smoke-go/main.go` drives the same wazero mechanism (ABI check,
  eval, refusal assert) and is wired into the shared harness
  (`conformance/harness/run.py`, path `go`).
- Full manifest parity for Go is proven two ways: the external repo's CI on
  every change, and the shared harness `go-native` path, which
  shallow-clones `Active-Official/active-go` and runs its full `go test`
  suite (see `bindings/go/run-tests.py`; SKIP when git/go or network is
  unavailable, never a silent pass).

## Appendix B status

Items 1, 2, 4 (opacity, agent-loop surface, ownership docs) hold in the
external repo by code inspection. Item 3 (harness integration) holds via
smoke-go in-tree plus external CI on the full manifest. Item 5 (CI) holds in
the external repo. Item 6 (matrix row) is this file plus the matrix entry.

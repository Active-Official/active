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

## D.001 packaging polish (2026-10-21, verified live)

- **Install path:** `go get github.com/Active-Official/active-go@latest`
  (library package, not a command; `go 1.25+`). Verified 2026-10-21:
  module `github.com/Active-Official/active-go`, `abiMajor = 1` gate
  in `active.go`, HEAD `f5bd6e2` matching the harness pin
  (`f5bd6e223dde0a0fd47ef3aa24343e9de1e4` in `run-tests.py`).
- **Version alignment:** the binding versions against the release
  artifact, not the Rust workspace (DERIVED decision above). Contract
  compatibility is the frozen C ABI v1 (`abiMajor = 1` gate at load);
  language-package versions track the workspace release line
  (currently 1.4.0) and the harness pin (`REPO_REF` in
  `bindings/go/run-tests.py`) moves deliberately with a green run.
- **Ownership docs:** `Active-Official/active-go` repo (README +
  this SPEC); opaque handles, close discipline, and the agent-loop
  surface documented there and asserted by `active_test.go`.
- **Conformance CI linkage:** external repo CI on every change plus
  the shared harness `go-native` path (pinned clone + full `go test`
  suite; SKIP openly without git/go/network, never a silent pass).

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

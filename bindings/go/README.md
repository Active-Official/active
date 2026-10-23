# Go Binding — `Active-Official/active-go`

The canonical Go binding lives in its own repository (wazero-based,
zero cgo, versions against the release artifact). Full rationale,
ownership rules, and conformance linkage: [`SPEC.md`](SPEC.md).

## Install

```sh
go get github.com/Active-Official/active-go@latest   # go 1.25+
```

## Agent loop (mirrors the other Tier-1 bindings)

```go
m, err := active.Load()      // abiMajor = 1 gate at load; mismatch fails closed
if err != nil { /* ... */ }
defer m.Close()              // opaque handle, explicit close discipline
v, err := m.Evaluate("port = 8080")
_ = v
_ = err
```

Secrets are redacted by the module itself (`***SECRET***`); there is
no separate redaction code to drift. Evaluate → check → repair loops
work exactly as in the Python and TypeScript reference loops
(`agent-kit/reference-loops/`).

## Conformance

The binding's own CI runs the shared `conformance/manifest.toml` on
every change (`active_test.go`). This repo additionally exercises it
two ways: `active-wasm/smoke-go/main.go` (in-tree wazero smoke, ABI
check, eval, refusal assert; harness path `go`) and the shared
harness `go-native` path (`bindings/go/run-tests.py`: pinned clone
plus full `go test`; SKIP openly without git/go/network, never a
silent pass).

## Versioning

Contract compatibility is the frozen C ABI v1 (`abiMajor = 1` gate).
The harness pin (`REPO_REF` in `run-tests.py`) moves deliberately
with a green run; language-package versions track the workspace
release line (currently 1.4.0).

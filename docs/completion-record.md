# Master Program completion record

**Date:** 2026-10-24
**Version:** 1.4.0 (workspace, all crates and language packages)
**Tree:** the commit that adds this record (see `git log -- docs/completion-record.md`)
**Programs:** Master (`PROPOSAL.md` / `CHECKLIST.md`) and Tooling
(`tooling/TOOLING-PROPOSAL.md` / `TOOLING-CHECKLIST.md`).
**Close-out register:** `master-program/GAPS-AND-DRIFT.md` (every row
marked FIXED / STILL PRESENT / ACCEPTED with evidence).

## 1. Declaration

Every Master Program phase (A, B, CA, SC, R, PE, M, GQ, CAP, CG, D, T2,
AK, AUD/OBS/EVID, FINAL) and every Tooling phase (LSP, FMT, LINT,
INSPECT, DEBUG, VIZ, REPAIR-UI, QUERY, EDITOR, QUALITY, DOCS-TOOLING,
TOOLING-FINAL) is complete. Steps already satisfied by existing code kept
that code and recorded evidence; deltas were implemented in place under
the baseline rule (no parallel rewrites).

## 2. Master Program evidence

| Phase | Result | Evidence |
|---|---|---|
| A — Tier-1 gate | Go/Python/JS/TS/Rust all Complete | `bindings/README.md` matrix; `conformance/evidence/harness.json` Tier-1 all pass |
| B — cross-implementation | differential gate green, codes+spans+values identical | `conformance/evidence/harness.json` (`parity: []`), `DIFFERENTIAL-OK` |
| CA — content-addressed blobs | `blob()` / `Blob` type / B00x / materialise | `spec/blob.md`, `active-core/tests/blob.rs`, `agent-kit/vectors/blob-clean.active` |
| SC — schema evolution | metadata, `removed_in`, C04x/E040, `active compatibility` | `spec/schema-evolution.md`, `active-core/tests/compat.rs` |
| R — repair algebra | goal/preserve/minimize, plan/apply, H006/H008 | `active-core/tests/repair.rs`, `active-core/src/convert/repair.rs` |
| PE — partial evaluation | `eval --path`, FFI project, incremental memo | `spec/partial-eval.md`, `active-core/tests/partial.rs` |
| M — mode | `active_mode`, E024, W013 at 80%, memo token | `spec/mode.md`, `active-core/tests/mode.rs` |
| GQ — graph queries | dependents/dependencies/shortest-path/secret-cone/CA-size | `active-core/tests/graph_queries.rs` |
| CAP — capabilities | `active_capabilities`, E025, 4 sandbox profiles, handshake | `active-core/tests/capability.rs`, `agent-kit/capability.schema.json` |
| CG — codegen | 6 targets validated by real ecosystems, byte-stable | `conformance/evidence/codegen.log` (`CODEGEN: ALL PASS`) |
| D — packaging | live 1.4.0 on crates.io, npm, PyPI; universal artifact; SBOMs | `docs/distribution.md`, `packaging/README.md`, registry checks |
| T2 — tier-2 formalization | matrix + per-language SPEC maps + smoke policy | `bindings/README.md`, `conformance/NORMALIZATION.md` |
| AK — agent kit | eval gate, 10 vectors, cone agreement, 14-task prompt pack | `conformance/evidence/agentkit-eval.json`, `prompts/results/round-master-t14.json` |
| AUD — audit trail | re-validatable redacted Active document | `spec/audit-trail.md`, `conformance/evidence` |
| OBS — tracing | opt-in, 4-field events, metrics mapping | `spec/observability.md`, `trace.rs` |
| EVID — evidence | harness, container, Miri, catalog, research summary | `conformance/evidence/README.md` |
| FINAL | suite green, docs/positioning, this record | this file + `gates.log` |

## 3. Tooling Program evidence

| Phase | Result | Evidence |
|---|---|---|
| LSP | 14 capabilities incl. formatting, symbols, plan actions, custom tokens | `active-lsp/tests/tooling.rs`, `spec/lsp.md` |
| FMT | canonical modes, idempotence, LSP wiring | `active-core/tests/format_roundtrip.rs`, `active-fmt/tests/cli.rs` |
| LINT | 10 rules, JSON + SARIF, catalog doc, CLI formats | `active-core/tests/lint.rs`, `docs/lint-rules.md` |
| INSPECT | path/value/taint/size/blob/schema/provenance explorer | `active-core/tests/inspect.rs`, `active-core/src/convert/value_inspect.rs` |
| DEBUG | step trace, breakpoints, replay, reduction | `active-core/tests/debug.rs`, `spec/debug.md` |
| VIZ | graph/taint/budget/schema/repair exports + static viewer | `active-core/tests/viz.rs`, `viz/index.html`, `spec/viz.md` |
| REPAIR-UI | browser model, partial apply, cherry-pick | `active-core/tests/repair.rs`, `spec/repair-ui.md` |
| QUERY | schema/mode/budget CLIs, committed JSON schemas | `docs/schemas/`, `conformance/evidence/output_schema_check.py` |
| EDITOR | VS Code status/commands/webview, Neovim Lua layer, gap notes | `editors/vscode/scripts/smoke.js`, `editors/neovim/lua/active.lua`, `editors/README.md` |
| QUALITY | fuzz CI job, property tests, benches, golden CLI tests | `.github/workflows/fuzz.yml`, `fuzz/Cargo.toml` (three libFuzzer targets), `active-lsp/tests/properties.rs`, `active-lsp/benches/analysis.rs`, `active-core/benches/tooling.rs` |
| DOCS-TOOLING | tooling guide, headless flows, descriptor features | `docs/tooling.md`, `docs/agent-headless.md` |
| TOOLING-FINAL | version alignment, VSIX + Neovim + docs artifacts | `release.yml` jobs, `packaging/test_package_neovim.py` |

## 4. GAP-CLOSURE status

**§1 exits:** all hold — shared harness green in CI; content-addressed
values and blobs shipped; repair with terminal states; inspector
productized; debugger; visualizers; capability descriptor (13 tools,
4 profiles); SARIF; Data Mode; evidence pack.

**§2 designs:** the interchange profile and tool-result envelope are
normative in `spec/agent-interchange-profile.md` and
`spec/tool-result-envelope.md`, with a conforming example
(`examples/agent-interchange/tool-result.active`) and conformance test
(`active-core/tests/agent_interchange.rs`). The playground now ships the
full generate → check → repair plan → partial apply → eval → full repair
loop in `playground/loop/` (`run.ps1`, `run.sh`, README transcript);
`cargo test -p active-cli --test loop_demo` drives the same pipeline
through the shipped binary so the demo cannot drift. The hosted
playground remains the browser evaluator; the headless walkthrough is in
`docs/agent-headless.md`.

## 5. Verification battery (this tree)

| Check | Result |
|---|---|
| `cargo build --workspace --locked` | pass |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `cargo test --workspace --locked` | pass (all suites) |
| Shared harness (`harness.json`) | `ok: true`, Tier-1 pass, `parity: []`, `secret_leak: false` |
| Zero-SKIP container harness | 28 pass, 0 skipped, 0 failed |
| Miri (trace + blob units) | pass, no UB |
| Codegen equivalence | `CODEGEN: ALL PASS` |
| Agent-kit eval | `ok: true`, 5 improved per loop, mean 1.5 |
| Catalog diff | 83 emitted, 84 catalogued, 0 missing (E009 intentionally unassigned) |
| Prompt pack | 14/14 copilot, 12/14 direct |
| Output schemas | 8/8 documents validate |
| Editor smoke | 6 commands, redaction probe pass |
| Neovim package determinism | identical hashes |
| Dual-push CI | recorded in `conformance/evidence/ci.log` |

## 6. Accepted, documented gaps

- Zed and JetBrains plugins are not implemented; the LSP host path and
  required packaging are documented in `editors/README.md`.
- Tier-2 bindings without a host toolchain SKIP openly in native CI and
  prove green in the all-toolchain container.
- Registry publication is live at 1.4.0; future releases follow the
  tag-driven release checklist.
- `master-program/` working copies are local-only by owner decision and
  are excluded from repository history.

**End of record.**

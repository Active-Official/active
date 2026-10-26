# Master Program completion record

**Date:** 2026-10-26
**Version:** 1.4.0 (workspace, all crates and language packages)
**Tree:** the commit that adds this record (see `git log -- docs/completion-record.md`)
**Programs:** Master (`PROPOSAL.md` / `CHECKLIST.md`) and Tooling
(`tooling/TOOLING-PROPOSAL.md` / `TOOLING-CHECKLIST.md`) from
`active-master-program 4`; the checked working copies are
`master-program/CHECKLIST.md` and `master-program/TOOLING-CHECKLIST.md`,
and the language design proposal is `docs/proposal.md`.
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
| CG — codegen | 6 targets emitted (JSON Schema, OpenAPI, TypeScript+zod, Go, Python, Rust); four validated by real ecosystems, Rust decode-checked in the log, OpenAPI shape-asserted in `codegen_check.py`, determinism stamped | `conformance/evidence/codegen.log` (`CODEGEN: ALL PASS`); byte-stability also pinned by `codegen_is_byte_stable_across_runs` |
| D — packaging | live 1.4.0 on crates.io, npm, PyPI; per-platform archives + WASM + VSIX; CycloneDX SBOMs and their `SHA256SUMS` attached to the `v1.4.0` release; universal assembly script in `packaging/assemble-universal.py` (maintainer-run) | `docs/distribution.md`, `packaging/README.md`, registry checks |
| T2 — tier-2 formalization | matrix + per-language SPEC maps + smoke policy | `bindings/README.md`, `conformance/NORMALIZATION.md` |
| AK — agent kit | eval gate, 11 vectors, cone agreement, 14-task prompt pack | `conformance/evidence/agentkit-eval.json`, `prompts/results/round-master-t14.json` |
| AUD — audit trail | re-validatable redacted Active document | `spec/audit-trail.md`, `conformance/evidence` |
| OBS — tracing | opt-in, 4-field events, metrics mapping | `spec/observability.md`, `active-core/src/trace.rs` |
| EVID — evidence | harness, container, Miri, catalog, research summary | `conformance/evidence/README.md` |
| FINAL | suite green, docs/positioning, this record | this file + `gates.log` |

## 3. Tooling Program evidence

| Phase | Result | Evidence |
|---|---|---|
| LSP | 14 capabilities incl. formatting, symbols, plan actions, custom tokens | `active-lsp/tests/tooling.rs`, `spec/lsp.md` |
| FMT | canonical modes, idempotence, LSP wiring | `active-core/tests/format_roundtrip.rs`, `active-fmt/tests/cli.rs` |
| LINT | 16 rules, project config, JSON + SARIF, catalog doc, CLI formats | `active-core/tests/lint.rs`, `docs/lint-rules.md` |
| INSPECT | path/value/taint/size/blob/schema/provenance explorer | `active-core/tests/inspect.rs`, `active-core/src/convert/value_inspect.rs` |
| DEBUG | step trace, breakpoints, replay, reduction | `active-core/tests/debug.rs`, `spec/debug.md` |
| VIZ | graph/taint/budget/schema/repair exports + static viewer | `active-core/tests/viz.rs`, `viz/index.html`, `spec/viz.md` |
| REPAIR-UI | browser model, partial apply, cherry-pick | `active-core/tests/repair.rs`, `spec/repair-ui.md` |
| QUERY | schema/mode/budget CLIs, committed JSON schemas | `docs/schemas/`, `conformance/evidence/output_schema_check.py` |
| EDITOR | VS Code status/commands/webview, Neovim Lua layer, gap notes | `editors/vscode/scripts/smoke.js`, `editors/neovim/lua/active.lua`, `editors/README.md` |
| QUALITY | fuzz CI job, property tests, benches, golden CLI tests | `.github/workflows/fuzz.yml`, `fuzz/Cargo.toml` (four libFuzzer targets: lexer, parser, interpolation, formatter), `active-lsp/tests/properties.rs`, `active-lsp/benches/analysis.rs`, `active-core/benches/tooling.rs` |
| DOCS-TOOLING | tooling guide, headless flows, descriptor features | `docs/tooling.md`, `docs/agent-headless.md` |
| TOOLING-FINAL | version alignment, VSIX + Neovim + docs artifacts | `release.yml` jobs, `packaging/test_package_neovim.py` |

## 4. GAP-CLOSURE status

### 4.1 §1 near-term exits

| Item | Exit evidence |
|---|---|
| 1.1 Cross-implementation gate | the shared harness is a required CI job on both remotes; `conformance/evidence/harness.json` (`parity: []`) and the zero-SKIP `container-harness.json` |
| 1.2 Content-addressed values + blob | `spec/blob.md`, `active-core/tests/blob.rs`, `agent-kit/vectors/blob-clean.active`; identity, equality, taint, budget, materialisation, and JSON-shape vectors |
| 1.3 Multi-step repair | `spec/repair.md`, `active-core/tests/repair.rs`, `active-core/src/convert/repair.rs`; single/multi-step, stuck (H006), unrepairable (H008), side-effect listing |
| 1.4 Inspector productized | `active-core/tests/inspect.rs`, `active-core/src/convert/value_inspect.rs`, `docs/schemas/inspect*.json`; library + CLI golden tests, no raw secrets |
| 1.5 Debugger/stepper/replay | `active-core/tests/debug.rs`, `spec/debug.md`; determinism + redaction tests, headless and LSP |
| 1.6 Visualizers | `active-core/tests/viz.rs`, `viz/index.html`, `spec/viz.md`; stable JSON/DOT/Mermaid, redaction checks |
| 1.7 Capability descriptor | `agent-kit/capability.schema.json`, `active capability`, `active-lsp` handshake; 20 tools, 4 profiles, unknown-major rejection |
| 1.8 SARIF + stable JSON | `active lint --format sarif`, `docs/schemas/lint*.json`, `active-core/tests/lint.rs`; SARIF validated in evidence |
| 1.9 Data Mode | `spec/mode.md`, `active-core/tests/mode.rs`; Config unchanged, Data vectors for progressive warnings and hard ceilings |
| 1.10 Evidence pack | `conformance/evidence/README.md` plus this tree's gates/harness/container/codegen/catalog/agent-kit logs |

### 4.2 §2 future-facing deliverables

| Item | Deliverable |
|---|---|
| 2.1 Agent-to-agent interchange profile | `spec/agent-interchange-profile.md`, three example envelopes under `examples/agent-interchange/`, and `active-core/tests/agent_interchange.rs` (fields, mode, handshake, redaction, directory vectors) |
| 2.2 "I already possess hash H" short-circuit | protocol note in the profile (hash short-circuit section) + agent-kit helpers/vectors |
| 2.3 Capability negotiation handshake | capability handshake in the profile and `active capability --profile`; agent-kit reference |
| 2.4 Deterministic explainer | `active-core/src/convert/explain.rs`, `active explain` (text/JSON), LSP hover texts |
| 2.5 Audit trail as Active data | `spec/audit-trail.md`, `--audit-trail`, re-validation in `active-cli/tests/cli.rs` (`audit_records_limits_and_revalidates`) |
| 2.6 Partial eval + memo as the agent path | `spec/partial-eval.md`, `eval --path`, incremental memo; `docs/agent-headless.md` |
| 2.7 Compatibility as queryable data | `active compatibility`, `spec/schema-evolution.md`, `active-core/tests/compat.rs` |
| 2.8 Stuck/unrepairable terminal state | H006 (stuck) and H008 (unrepairable) in `active-core/src/convert/repair.rs` + repair tests |
| 2.9 Secret-safe tool-result envelope | `spec/tool-result-envelope.md`, `examples/agent-interchange/tool-result.active`, conformance test |
| 2.10 Playground full agent loop | `playground/loop/` (`run.ps1`, `run.sh`, `transcript.txt` captured from the shipped binary) and `active-cli/tests/loop_demo.rs` |

## 5. Verification battery (final tree)

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
| Catalog diff | 89 emitted, 90 catalogued, 0 missing (E009 intentionally unassigned) |
| Prompt pack | 14/14 copilot, 12/14 direct |
| Output schemas | 9/9 documents validate (`output-schema.log`) |
| Editor smoke | 6 commands, redaction probe pass |
| Neovim package determinism | identical hashes |
| Dual-push CI | recorded in `conformance/evidence/ci.log` |
| Independent review | DeepSeek V4.1 Flash (BYOK) at `--reasoning-effort max` for rounds 12-76 and `xhigh` for the round-77 fresh review. Round 73 re-reviewed the complete goal diff and found 12 issues (2 critical, 1 high, 6 medium, 3 low); every one was fixed at the cause with a regression test on the shipped path. Round 74 verified 11 as FIXED and flagged F6 for a missing discriminating test; round 75 verified the added docgen bound (0.25 s committed vs 3.80 s with the old body). Round 76 closed the last leak (the linter's env-equal deprecated reason) and verified it in text, JSON, and SARIF. A fresh full-diff review at `b96509f` (round 77) found 7 further issues (2 high, 3 medium, 2 low), all fixed at `fcea0d3` and verified fixed by round 77b with no new findings requested. Final verdict: zero open findings (`conformance/evidence/review.log`; detail in `docs/security-hardening.md`) |

### Final CI (2026-10-26)

Both remotes are green on the final content tree `fcea0d3` and on the
two evidence stamps `1b5ce29` (review logs) and `1fdeaf5` (gate and CI
table). CI and Conformance pass on all three trees. CodSpeed's verdict
for `fcea0d3` itself is `cancelled` (superseded before it reported), so
the CodSpeed proof is the `1b5ce29` run, which is the same code with
documentation-only changes. Fuzz smoke is not path-triggered by an
evidence-only commit; it passed twice on the content tree.

| Tree | Remote | CI | Conformance | CodSpeed | Fuzz smoke |
|---|---|---|---|---|---|
| `fcea0d3` | origin | success (40333418737) | success (40333418591) | cancelled (40333418771, superseded) | success (40333419116, 40333502494) |
| `fcea0d3` | official | success (40333428157) | success (40333428214) | cancelled (40333428250, superseded) | success (40333428137, 40333507485) |
| `1b5ce29` | origin | success (40335637519) | success (40335637847) | success (40335637525) | not triggered (same code) |
| `1b5ce29` | official | success (40335643045) | success (40335644400) | success (40335642928) | not triggered (same code) |
| `1fdeaf5` | origin | success (40343204543) | success (40343204510) | success (40343204597) | not triggered (same code) |
| `1fdeaf5` | official | success (40343210622) | success (40343211064) | success (40343210683) | not triggered (same code) |

Earlier green stamps: `b96509f` (docs and evidence), `b95ea27`
(content), and `f6efd0d` (content) were green on CI, Conformance, and
CodSpeed on both remotes; run IDs are in the goal close-out and
`conformance/evidence/ci.log`.

The Ubuntu conformance job also builds the OCaml binding; `ocaml_stubs.c`
includes `<caml/fail.h>` so `caml_invalid_argument` is declared under
modern compiler warnings. The verdict summary is
`conformance/evidence/ci.log`; the goal close-out records the run IDs.
## 6. Post-loop audit additions (2026-10-26)

An independent audit re-read both program checklists against the tree
after the round-72 verdict. These surfaces were added and tested in the
same close-out (full detail in `docs/security-hardening.md`):

- **Linter:** L011-L016 (requires discipline, agent annotations,
  ambiguous repair, canonical order, mixed syntax, include hygiene) and
  `.active-lint.toml` project configuration (disable list, severity
  overrides, upward discovery, `--config`). The catalog is 16 rules.
- **Debugger:** `budget:<percent>` / `budget:nodes:<n>` breakpoints; the
  reduction report exports env names and the effective budget;
  `active/debug/steps` serves the trace over LSP.
- **Formatter:** a fuzz target plus a deterministic mutation sweep (which
  found and fixed a `secret.env` name round-trip bug), and a CI
  `active fmt --check` gate over `tests/fmt/`.
- **Repair browser:** code/severity/path filters; the plan stays whole.
- **Visualizer:** `compatibility` block (version floors, deprecated
  hints, removal schedule) in `viz schema`.
- **Language server:** `active/capability`, `active/explain`, and
  `active/debug/steps` custom methods; hover appends the canned fix
  guidance.
- **Capability descriptor:** 20 tools (was 13), matching profiles, CLI
  mapping test, Python authority test.
- **Interchange:** three example envelopes, directory vectors, and the
  §7 hash short-circuit with executable helpers and a pinned vector.
- **Docs:** README Config/Data modes section and evidence index,
  `docs-site/src/reference/modes.md`, `spec/repair.md`, playground
  transcript, CI artifact retention.

## 7. Accepted, documented gaps

- Zed and JetBrains plugins are not implemented; the LSP host path and
  required packaging are documented in `editors/README.md`.
- Tier-2 bindings without a host toolchain SKIP openly in native CI and
  prove green in the all-toolchain container.
- Registry publication is live at 1.4.0; future releases follow the
  tag-driven release checklist.
- Branch protection on the two remotes is an owner-controlled setting;
  the workflows gate every push and PR, and the final CI verdicts are
  recorded in `conformance/evidence/ci.log`.
- Secret analysis is deliberately conservative: a material schema
  default that a resilient rename preempts can keep the healed key
  marked material (the value is hidden, never exposed;
  spec/secrets.md).
- `master-program/` working copies are local-only by owner decision and
  are excluded from repository history.

**End of record.**

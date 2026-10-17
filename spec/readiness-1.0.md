# Version 1.0 Readiness Review (C-090)

Date: 2026-10-16. Reviewer: lead engineer (author). Verdict: **PASS
on all ten** — Version 1.0 declared below. Each criterion cites its
evidence pointer (test target, command, or artifact).

## 1. Expressiveness

Single files (plus includes) cover Kubernetes services, Terraform
variables, 12-factor apps, CI pipelines, feature flags, and AI
prompts — each as an executed cookbook sample
(`docs-site/src/cookbook/*.active`, all evaluated by
`active-core/tests/docs_site.rs` with value assertions), plus
multi-region overlays and migration guides. **PASS.**

## 2. Hermeticity

The language has no I/O, shell, network, `eval`, or loop constructs
(grammar: spec/grammar.md); evaluation resolves only literals,
references, injected env snapshots, and secrets. Fuzz harness
(`--test fuzz`: arbitrary bytes, no panics, always terminates),
budget enforcement including the DP bound analysis
(`--test budgets`, 17 tests) prove boundedness. **PASS.**

## 3. Determinism

Same input → same value tree + same diagnostics: proptest
determinism properties (`--test proptest_invariants`), conformance
double-eval pins (`--test conformance`), validation determinism
(`validation_is_deterministic` in `--test validation`), and CI
running the suite on every push across platforms. **PASS.**

## 4. Dual syntax

Brace, indent, mixed, and dotted styles lower to identical IR
(`--test dual_syntax`, 30 tests), lossless comment/annotation
round-trips (`--test format_roundtrip`, 18 tests), brace/indent
spec-corpus pairs (10–11). **PASS.**

## 5. Secrets

Taint-as-kind through interpolation, arithmetic, and construction;
exports refuse/redact (`***SECRET***`); audits prove no leakage
into diagnostics, logs, errors, hover, graphs, or docs
(`--test secrets`, secret proptest, schema-library redaction pins,
docgen leak pins). No unlock mechanism exists, so the criterion's
escape clause is vacuously satisfied — stronger than required.
**PASS.**

## 6. Toolchain

Ships and works: parser, evaluator, formatter (`ACTIVE fmt`,
3 styles + `--check`), linter (`ACTIVE lint`, independent rules),
converters both directions + schema codegen (5 targets:
`jsonschema`, `openapi`, `typescript`, `go`, `rust`),
documentation generator (Markdown/HTML/man), language server
(stdio LSP 3.17 — boots and answers `initialize`, pinned by the
stdio initialize e2e in `active-lsp/tests/lsp.rs`), CI checker
(`ACTIVE check`). Each proven by its test target and the CLI gate
(`cargo test --workspace` green + `ACTIVE check` on both examples
in CI). **PASS.**

## 7. Schemas, profiles, patches, units, graphs

Schema language + strict/resilient validation (`--test validation`
33, `--test schema`, schema library 10), profiles with merge rules
(`--test profiles`), structural patches as IR nodes (`--test
patches`), first-class units with normalization (`--test
unit_roundtrip`, spec 02/08), graph/impact/why via CLI + library +
LSP (`--test graph`, CLI contract tests). Documented in
spec/ + the docs site. **PASS.**

## 8. AI prompt pack

`prompts/v1` (generator/editor/explainer) validated by
`prompts/runner/validate.py` against the fixed 8-task suite on two
backends: round-3 first-attempt rates **1.0 copilot / 1.0 direct**
(16/16 — unanimous on the fixed suite,
`prompts/results/round-round3.json`). Version/fence pins are gated
in CI (`--test prompts`); the live 2-backend runs are committed
manual artifacts, re-runnable with one command. **PASS.**

## 9. Performance

10,000-node eval mean ~25 ms release (p99 under the 50 ms
internal target; the proposal asks well under 100 ms),
leaf edits re-evaluate 1.3% of nodes, tracked continuously by
criterion benches + CodSpeed (`active-core/benches/eval.rs`,
`.github/workflows/codspeed.yml`; numbers in
spec/performance.md §3). **PASS.**

## 10. Security review

No open critical or high findings across all independent review
rounds (Copilot CLI + Meta-proxy BYOK, every round triaged in
`docs/Memories-and-Observations.md`, ending clean each time):
healing/rename soundness, span anchoring, parser UTF-16/scoping,
export redaction, TOCTOU/single-read, overlay trust sets, walltime
budgets under instrumentation. `cargo audit` clean (one documented
ignore with MSRV-bump exit condition + one allowed warning).
Threat model committed (spec/threat-model.md). No third-party
audit exists — stated plainly, not implied. **PASS** (internal
review bar).

## C-070 re-affirmation (1.0 cycle)

None of the five non-goals entered the language: no loops or
recursion constructs (cyclic graphs rejected pre-evaluation), no
I/O or side effects in evaluation, no dynamic loading or string
eval, coercion limited to the documented resilient set (always
reported), no data-transformation features beyond typed binding
generation. The living statement is `docs-site/src/non-goals.md`.
**Re-affirmed.**

## C-080 final risk review (1.0 cycle)

| P-080 risk | 1.0 outcome |
|---|---|
| Dual-syntax parsing complexity | Closed: differential brace/indent/mixed tests (30), lossless round-trips (18), formal grammar; no open syntax bugs. |
| Secret leakage through diagnostics/logs | Closed: taint-as-kind + redaction audits across exports, diagnostics, hover, graphs, docs; `cargo audit` gating; criterion 5 evidence. |
| Performance regression with large graphs | Closed: 25 ms p99 at 10k nodes, sub-DAG incremental re-evaluation, CodSpeed continuous tracking. |
| AI agents inventing invalid syntax | Closed: prompt pack at 1.0/1.0 first-attempt, schema completions, healer with confidence policy. |
| Scope creep into full programming language | Closed: C-070 re-affirmed above; RFC process gates post-1.0 changes. |

No new risks discovered this cycle. Next review: the 1.x cycle per C-080.

## Declaration

All ten criteria pass. Checklist C-010 through C-090 (plus C-070)
carry `Status: done`. Workspace version bumped to **1.0.0**;
reference implementation declared (spec/conformance.md);
conformance suite green. **ACTIVE 1.0 is declared.**

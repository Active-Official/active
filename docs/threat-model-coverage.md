# Threat-model coverage (spec/threat-model.md → tests / residual risks)

Each model item names its proof. "Test" = committed test that fails if the
protection regresses. "Residual" = accepted risk written in the model itself.

// markdownlint-disable MD013

## 1. Assets

| Asset | Proof |
|---|---|
| Secret values never in exports | Test: `convert.rs::export_redacts_secrets_uniformly`, `redaction_paths.rs::json_yaml_toml_exports_redact`, `proptest_invariants.rs::secrets_never_leak_into_exports`, ported accept fixtures (canonical JSON carries only markers) |
| Secret values never in diagnostics/errors | Test: `secrets.rs::diagnostics_audit_never_shows_raw_secrets`, `convert.rs::import_hard_errors_withhold_source_excerpts` |
| Never in logs | Test by construction: the only stdout/stderr paths are CLI diagnostic messages (covered above) and evaluated outputs (covered above); no tracing/log framework exists in the workspace |
| Never in hover/hints/docs/graphs | Test: `lsp.rs::hover_redacts_secrets`, `redaction.rs` (completions, inlay, diagnostics, code actions, graph snapshot/impact/why), `redaction_paths.rs::docgen_all_formats_redact`, `graph.rs` name-only labels, `cli.rs` doc/graph goldens |
| Never in lockfiles | Test: `lock.rs::manifest_records_plain_secret_and_unset_vars` (hash only), `transitive_secret_flows_are_secret_bound` |
| Never in generated bindings | Test: generators render types, never values (`generate.rs`); schema authors cannot inject values (model §3.4) |
| Config integrity (hashes) | Test: `lock.rs` content-hash stability (`redacted tree, same hash` case) |
| Overlay signatures | Test: `sign.rs` + `cli.rs::keygen_sign_verify_round_trip` |
| Provenance accuracy | Test: `lock.rs` manifest shape (plain/secret/unset, includes, profile) |
| Budgets bound every dimension | Test: `budgets.rs` (fail-closed X001–X006, both modes) |

## 2. Actors and trust — enforced by

| Rule | Proof |
|---|---|
| Include paths literal/relative, no escapes (E015) | Test: `includes.rs` |
| Include cycles/depth rejected (E016) | Test: `includes.rs` |
| Private includes never leak bindings | Test: `includes.rs` (private cases) |
| Signed includes with `--trust-keys` (E032) | Test: `sign.rs`, CLI trust-keys paths |
| Patches overlapping `@ai_immutable` fail | Test: `annotations.rs::immutable_keys_reject_all_patch_ops` |
| Schemas constrain, never inject | Test: `schema.rs` + `generate.rs` (defaults render, dynamic bounds skipped) |
| AI writes go through validated parses | Test: `prompts.rs` + `prompts/runner/validate.py` gate |

## 3. Accepted residual risks (written in the model, not defended further)

1. Secret routed through PLAIN `env`/literal (model §3.2): social mitigation
   (L004 flags probable literals). No test can distinguish sensitive plain
   data — recorded here as accepted.
2. Incremental memo holds same-process plaintext (model §3.2): three
   properties pinned — salted keys, shape-only `Debug`, 64-bit FNV-1a width.
   Covered by `incremental.rs` telemetry/shape tests; per-memo 2^64 forging
   cost documented as not defended further.
3. Signature scope limits (model §3.5): authorship only, no revocation, no
   timestamps, bearer key files. Keygen `0600` pinned
   (`cli.rs::keygen_restricts_secret_permissions`); rotation/replay posture
   is documentation, verified by reading §5, not by test.
4. Plain values DO export through hostile includes (model §3.1): by design
   (includes are data); secret exfiltration specifically is blocked (S004,
   tested in `secrets.rs`).

## 4. Supply chain

`cargo audit` (CI gate, G6), committed lockfile, MSRV 1.85 gate,
`cargo package` file-inclusion gate (`packaging.rs`), signed release
binaries + attestations (release.yml, C-056).

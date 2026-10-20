# Shared conformance harness (CHECKLIST Step 0.004, PROPOSAL sections 2.4 and 5.1)

`run.py` executes the same conformance manifest through every available
implementation and prints one unified JSON report. Run from anywhere:

```bash
python conformance/harness/run.py [--out report.json]
```

Exit code is 0 only when every implemented path passes. Placeholder paths
report `not-implemented` and never count as passes. Paths that time out
(900s per path) or whose binary is missing report `fail` with details.

Prerequisites: Rust toolchain with the `wasm32-unknown-unknown` target for
reference and C ABI paths, Node 20+ for the WASM path, Go toolchain for the
wazero smoke path. Run `python conformance/harness/run.py --self-check`
first to validate the tally parser itself.

## Normalization rules

- **Codes strict.** Diagnostic codes compare exactly; any mismatch fails the path.
- **Spans advisory (tier G-017).** Span drift is reported, never failing.
- **Secrets fail closed.** The fixture raw value `hunter2` must appear in no
  runner output. Any sighting fails the whole run.
- **JSON key order ignored.** Objects compare after parsing, never as raw text.

## Adding a binding path

Append a `(name, argv, cwd, tally_regex)` entry to `PATHS` in `run.py`,
remove the name from `PLACEHOLDERS`, and wire the binding's runner to print
one tally line the regex can match. The path must also satisfy Appendix B
(Definition of Done) before it counts.

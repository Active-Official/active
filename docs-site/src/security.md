# Security deep-dive

Configuration holds secrets, so the language treats secrecy as a
kind of value, not a convention.

**Taint-as-kind.** `secret(…)` marks a value; taint propagates
through interpolation, arithmetic, and table construction. Tainted
data refuses export (`S004` on interpolation, redaction everywhere
else), and audits prove raw values never reach diagnostics, logs,
errors, hover text, graphs, or generated docs. A secret in a
*plain* string is the author's bypass — lint rule `L004` flags
probable literals, but the toolchain cannot distinguish sensitive
plain data.

**Hermetic evaluation.** No I/O, shell, or network during eval;
environment arrives as an injected snapshot. Budgets bound every
dimension (nodes, wall-clock default 100 ms, depth, array length,
key totals, materialized size) — exceeding any is a hard `X0xx`
error, which also kills billion-laughs shapes before they
materialize.

**Provenance and signing.** `ACTIVE lock` binds content hash +
provenance + toolchain version. Overlays sign detached Ed25519
sidecars verified against trusted key sets (`ACTIVE keygen` /
`sign` / `verify`); releases add SHA-256 checksums, build
attestations, and optional `ACTIVE-sign` sidecars. Full model:
`spec/threat-model.md`.

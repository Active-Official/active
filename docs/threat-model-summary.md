# Active threat-model summary (G-011c, reviewer-aimed)

Full model: `spec/threat-model.md`. This page states what reviewers check.

## Assets

Secrets (taint end to end, redacted exports), evaluation integrity
(budgets, determinism), and provenance accuracy (lockfiles, signatures).

## Trust boundaries

- Documents are untrusted input; the evaluator is hermetic (no shell,
  network, filesystem) and budgeted (nodes, depth, size, wall-clock).
- Includes load from the filesystem (or host callbacks across FFI) with
  parent-escape controls and optional signature trust sets.
- Environment values come from a trusted operator; secret material travels
  only through `secret()`/`secret.env()`, never plain bindings.

## What we promise

No silent coercion, no unbounded evaluation, no secret leakage through
any shipped exit (values, JSON, diagnostics, spans, logs, FFI strings,
WASM envelopes). Every promise has a failing-if-broken test; ask for the
test by name if you doubt a row.

## What we do not promise

Protection against a malicious operator with write access to documents or
trust sets; availability under infinite host resources (budgets bound the
engine, not the machine); secrecy of key names, table shapes, or
diagnostic paths (only secret values redact, and messages never carry
evaluated values at all).

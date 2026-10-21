# Binding stability and deprecation policy (CHECKLIST 3.007)

Bindings are user-facing API. This policy keeps them stable while the
language grows.

## Stability promise

- A Tier-1 binding's public API is stable within its major version.
  Breaking changes require a major bump of that binding and a migration
  note in `bindings/README.md`.
- The frozen C ABI v1 never breaks: new exports are additive only, and
  every binding gates on `active_abi_version()` at load, failing closed
  on major mismatch. A binding never needs an emergency release because
  the core moved.
- Diagnostic codes are never reused (G-011b). Bindings that match on
  codes keep working across core releases by construction.

## Deprecation path

1. Announce in the binding README and the changelog: what is deprecated,
   its replacement, and the removal version (never earlier than the next
   major of that binding).
2. Keep both paths working for at least one minor release. The deprecated
   path emits a deprecation diagnostic or warning, never silent behavior
   change.
3. The conformance harness keeps exercising the deprecated path until
   removal, so drift cannot hide behind a deprecation flag.

## Support matrix maintainership

- `bindings/README.md` is the single source of truth. Every binding
  ships a `SPEC.md` linked from its matrix row; rows without a live
  SPEC are marked Planned and carry no other status.
- Status changes land with the code that earns them, in the same commit:
  a green harness path plus tests moves a row; prose alone never does.
- Tier definitions live in `bindings/README.md` and change only by RFC.
- The matrix is re-verified on every release: unmaintained Tier-1
  bindings drop to Tier-2 with a dated note rather than rotting green.

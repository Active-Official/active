# Schemas and validation

Schemas declare the contract; validation runs after evaluation, in
both modes:

```solid
schema = { closed = true, port = Int(min = 1, max = 65535) }
port = 8080
```

Types include `Int`, `Float`, `String`, `Bool`, `Duration`,
`Bytes`, `Enum`, `Array<T>`, and (possibly closed) tables, with
bounds, lengths, patterns, and defaults (`Int(default = 3)` fills
missing keys under resilient mode with a `W005` note).

Two modes, one rule — strict never silently changes the tree:

- **Strict** (CLI default): every violation fails, fully collected.
  A typo'd key in a closed table errors with a suggestion
  (`unexpected key 'prt' …; did you mean 'port'?`, code `T005`)
  plus a machine-applicable repair attachment. Nothing is renamed.
- **Resilient** (`--resilient`): confident repairs apply and are
  reported — `prt` renames to `port` (`H005` warning), `"5s"`
  coerces to a duration (`H007` warning), defaults fill in
  (`W005`). The tree, the diagnostics, and the exit code stay
  deterministic.

Closed tables reject unknown keys; open tables allow extras (kept
in the tree, never dropped). Every
violation carries a stable code (`T002` type, `T003` constraint,
`T004` missing, `T005` unexpected) with a precise span.

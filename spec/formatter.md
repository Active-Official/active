# SOLID Production Formatter (C-040/C-041)

The formatter is deterministic source-to-source normalization: parse
(without includes resolution, without evaluation) then re-emit in a
canonical layout. It never evaluates, never heals, and never touches
the value tree — formatting cannot change what a document means.

## 1. Styles

- **Indent** (default per P-040's human-indented default): `key:`
  headers with two-space indentation, `key = value` leaves. Empty
  tables emit as `key = {}` (a bodyless block is inexpressible
  otherwise).
- **Brace** (`--style brace`): `[name]` headers at top level,
  `key { … }` nested blocks, `key = value` pairs.
- **Preserve** (`--style preserve`): re-emit in the source's
  predominant style, canonical layout within. Mixed-style documents
  normalize to whichever style holds the majority of table blocks.

Guarantees (tested, spec/dual-syntax.md §8): idempotence
(`fmt(fmt(d)) == fmt(d)`), byte-for-byte round-trip of canonical
documents, comment/annotation/semantic-block survival in every mode,
and cross-style tree equality (both styles parse to the same IR).

## 2. `solid fmt` contract

```
solid fmt [--style brace|indent|preserve] [--write] [--check] <file>...
```

At least one file is required; an empty file list is an error
(never a silent clean bill). The default style is `indent`.

- **Stdout mode** (default, exactly one file): print the formatted
  document to stdout. Multiple files without `--write`/`--check`
  are an error — concatenated outputs would be ambiguous.
- **`--write`**: rewrite each file in place with its formatted form.
  Prints `formatted <file>` per rewritten file. Files already
  canonical are left untouched (mtime preserved).
- **`--check`**: write nothing. Print `would reformat: <file>` per
  file whose canonical form differs, and exit 1 if any differ (0
  when all are clean). This is the CI mode.
- `--write` and `--check` together are an error (contradictory).
- Exit codes: `0` = clean / written; `1` = differences found
  (`--check`), parse errors, I/O errors, or flag misuse. Parse
  errors report diagnostics like `solid check` and abort before any
  write — a file that does not parse is never reformatted, and when
  several files are given, files before the failure keep their
  results (writes already done stay done).
- Formatting parses with includes **unresolved**
  (`resolve_includes = false`): the composition structure of the
  input survives byte-for-byte in intent (include statements stay
  where they are). Healing repairs do not run — fmt uses strict
  parse options, so a file needing repair fails instead.

## 3. `solid-fmt` binary parity

The standalone `solid-fmt` binary keeps its single-file behavior
and gains `--check` with identical semantics (no writes, names
differing files, exit 1 on differences). `--write` keeps working.

## 4. LSP readiness (C-043 consumes this)

`solid_core::fmt::format(&Module, Style)` plus `Style::Preserve`
detection is the whole API the language server needs for format-on-
save and range-formatting-later: parse (recovering, so broken files
still produce a tree), format, apply as a full-document edit. No
formatter changes are expected for LSP work.

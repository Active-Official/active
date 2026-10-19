# Getting started

Install the toolchain (Rust 1.85+), then evaluate your first file.

```bash
cargo install active-cli active-fmt active-lsp
```

Write `hello.active`:

```active
name = "world"
greeting = "hello ${name}!"
```

Evaluate it:

```bash
ACTIVE eval hello.active
```

```json
{
  "name": "world",
  "greeting": "hello world!"
}
```

`ACTIVE check` validates without printing (`OK` means clean, exit 0).
`ACTIVE fmt` formats, `ACTIVE lint` lints, `active-lsp` serves editors.
Every command below works on the files in this book — each cookbook
and migration sample is executed by the test suite, so nothing here
rots.

## First-run checklist (under 10 minutes)

```bash
active check examples/complete.active   # expect: OK, exit 0
active eval examples/complete.active    # expect: JSON value tree
```

Then open `examples/complete.active` in your editor with the Active
extension: hover a unit, a secret, and a `let` binding. Finally, break
something on purpose: change a duration to a bare number, or reference a
name that does not exist, and read the diagnostic. It names the exact line
and column plus a code you can look up. That loop (write, check, read,
fix) is the whole workflow.

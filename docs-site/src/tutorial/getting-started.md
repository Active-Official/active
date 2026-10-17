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

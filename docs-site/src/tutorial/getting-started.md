# Getting started

Install the toolchain (Rust 1.85+), then evaluate your first file.

```bash
cargo install solid-cli solid-fmt solid-ls
```

Write `hello.solid`:

```solid
name = "world"
greeting = "hello ${name}!"
```

Evaluate it:

```bash
solid eval hello.solid
```

```json
{
  "name": "world",
  "greeting": "hello world!"
}
```

`solid check` validates without printing (`OK` means clean, exit 0).
`solid fmt` formats, `solid lint` lints, `solid-ls` serves editors.
Every command below works on the files in this book — each cookbook
and migration sample is executed by the test suite, so nothing here
rots.

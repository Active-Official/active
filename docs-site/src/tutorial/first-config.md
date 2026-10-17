# Your first config

Tables group keys. Write them with brackets or indentation — the
value tree is identical either way:

```active
[server]
host = "localhost"
port = 8080
```

```active
server:
  host = "localhost"
  port = 8080
```

Values carry types: integers, floats, strings, booleans, arrays,
durations (`30s`), byte sizes (`512MiB`), and rates (`100/req/s`).
References point at other keys and always resolve before use —
`let` bindings are visible everywhere (bare top-level keys are
visible only at the top level):

```active
let base_port = 8000
[server]
port = base_port + 80
```

`let` bindings compute shared values without exporting them.
`env.NAME ?: default` reads the environment from a snapshot taken
at startup (injectable for tests and servers): unset names fall
back to the written default, so evaluation stays deterministic.
`secret(...)` marks tainted values that redact on every export
path. Interpolation `"...${expr}..."` supports full expressions
with `\${` escapes — nesting works through escaped inner strings
(`"outer ${ \"inner ${name}\" }"`); every sub-expression becomes a
dependency edge, so cycles like `a = b` / `b = a` are rejected up
front with a stable `S001` diagnostic, never half-evaluated.

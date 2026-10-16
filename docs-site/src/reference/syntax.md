# Syntax reference

The authoritative grammar is `spec/grammar.md` (complete enough for
a second implementer); this page is the working summary.

**Documents** are sequences of statements: `let` bindings, tables,
key-values, includes, profiles, patches, and schema blocks.
**Tables** use `[name]`, dotted `[a.b.c]`, brace `name { … }`, or
indentation `name:` — all four lower to the same IR, freely mixed.

**Values**: `null`, booleans, integers, floats, `"double"` and
`'single'` strings, `"""triple"""` / `'''triple'''` multi-line
blocks (common-indent stripped), arrays `[1, 2]`, inline tables
`{ a = 1 }`, durations (`30s`, `5m`, `1.5h`), byte sizes (`512MiB`,
`1GiB`), rates (`100/req/s`).

**Expressions**: arithmetic and boolean operators with standard
precedence, bare-identifier references (dependency edges —
cycles rejected as `S001`), `${…}` interpolation over full
expressions (nesting through escaped inner strings, `\${` escapes),
`env.NAME ?: default`, `secret(…)` tainting.

**Overlays**: `include "path"` (literal paths, relative, no parent
escape, no cycles), `profile "name" { … }` + `active_profile`,
`@patch` / `@remove` / `@rename` applied post-evaluation.

**Schemas**: `schema = { port = Int(min = 1, max = 65535) }` inline
(or shared via `include`); `Int Float String Bool Duration Bytes
Enum Array<T>` closed/open tables; defaults fill under resilient
mode.

**Annotations**: `# @intent: "…"`, `# @constraint: value >= 1`,
`# @ai_immutable: true` above bindings; `@prompt` / `@policy` /
`@template` tagging multi-line strings. Comments are `#` to
end-of-line and survive formatting round-trips.

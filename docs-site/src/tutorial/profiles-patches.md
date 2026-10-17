# Profiles and patches

One base file, many environments. Profiles select overlays;
patches rewrite structure — both participate in the dependency
graph, apply after evaluation, and validate like written values:

```active
[server]
port = 8080

profile "prod" {
  [server]
  port = 80
}
active_profile = "prod"
```

Merge rules: last-wins scalars, deep-merge tables, conflict
diagnostics when types disagree. Structural patches go further —
document-level operations with dotted-path targets (`@patch` sets,
`@remove` deletes, `@rename` takes a dotted source and a plain new
name in the same parent):

```active
[server]
port = 5
debug = true
old_key = 1
@patch("server.port", 9090)
@remove("server.debug")
@rename("server.old_key", "new_key")
```

`include "shared/base.active"` pulls in literal-path fragments
(relative, never escaping the parent, cycles rejected; private
includes never leak bindings) — the unit of reuse behind the
[standard schema library](../reference/schema-library.md).
`ACTIVE graph` shows the dependency shape; `ACTIVE impact <key>` /
`ACTIVE why <key>` answer single-file dependent/dependency queries
for any key.

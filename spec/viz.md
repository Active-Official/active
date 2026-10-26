# Visualizer exports (TOOLING §6)

Every emitter returns redacted JSON. Taint and budget carry
`version: 1`; graph and repair are fixed shapes.

## `active viz <kind> --from <file>`

| Kind | Output | CLI formats |
|---|---|---|
| `graph` | `{nodes: [{id,label}], edges: [{from,to}]}` | `json` (default), `mermaid`, `dot` |
| `taint` | `{version, nodes, edges, sources, export_points}` | `json` |
| `budget` | `{version, mode, limits, usage, headroom, warnings}` | `json` |
| `schema` | INSPECT v2 document plus `mode` and `compatibility` | `json` |
| `repair` | `RepairOutcome` without `repaired_source` (goal, steps, clean/stuck/unrepairable, final_codes, findings) | `json` |

### Taint

Nodes are every path whose subtree contains material, where material
includes a plain value used as a `secret(...)` locator and anything
built from one (the same structural model as `inspect` and step
traces); `secret: true` marks a material leaf. Edges run containment
parent → child between named paths that were emitted as nodes: a
transform can materialize a child under a harmless parent, so an edge
to an unemitted parent is omitted. The document root has no
path, so top-level nodes have no incoming edge. `sources` and
`export_points` are the material paths; no value, locator, or payload
is emitted.

### Budget

`limits` is the effective `Budget`; `usage` is the static observation
(nodes = arena length, depth and keys from the evaluated tree, sizes
from canonical redacted JSON); `headroom` is `limit - used` (null when
the limit is disabled with `0`); `warnings` lists W013 codes recorded
for the run. Walls are host-time and appear as limits only.

### Repair

The plan is produced by the shipped engine with the caller's goal, step
cap, and file context, so `viz repair` and `active repair` agree by
construction (including includes). Directives are emitted verbatim:
planning withholds a fill that would copy material, so structural paths
stay applicable. `repaired_source` is omitted: it can
contain an authored secret literal, and this export is display data.

## Static viewer

`viz/index.html` + `viz/viewer.js` render any export from a file picker:
budget tables, taint graph lists, schema field tables, or step lists.
Zero dependencies, zero network, no evaluation — the JSON is already
redacted.

## Secret guarantee

Tests pin that the serialized taint, budget, schema, and repair exports
never contain fixture secret material, and that the graph emitters
carry names only.

### Schema and compatibility

`schema` is the INSPECT v2 document (field tree, types, constraints,
defaults, descriptions, `deprecated`, `removed_in`, `requires`,
`version`/`min_reader`/`min_writer`) plus two derived keys:

- `mode` — `config` or `data`.
- `compatibility` — the migration view:
  - `schema_version`, `min_reader`, `min_writer` (null when the schema
    declares none);
  - `deprecated` — `[{path, hint}]`, sorted by path, where `hint` is
    the authored `deprecated` message (`"deprecated"` for a bare
    `deprecated = true`);
  - `removals` — `[{path, removed_in, hint}]`, sorted by path, with a
    generated `migrate before <version>` hint.

Paths and schema prose only: no evaluated value enters the view. The
whole document is still passed through the material profile by the CLI,
so schema prose that equals a host value redacts exactly like every
other prose surface.

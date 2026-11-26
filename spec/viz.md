# Visualizer exports (TOOLING §6)

Every emitter returns redacted JSON. Taint and budget carry
`version: 1`; graph and repair are fixed shapes.

## `active viz <kind> --from <file>`

| Kind | Output | CLI formats |
|---|---|---|
| `graph` | `{nodes: [{id,label}], edges: [{from,to}]}` | `json` (default), `mermaid`, `dot` |
| `taint` | `{version, nodes, edges, sources, export_points}` | `json` |
| `budget` | `{version, mode, limits, usage, headroom, warnings, cost_model, charge}` | `json` |
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
the limit is the `UNLIMITED` sentinel, `18446744073709551615`, which the
CLI's `--max-* 0` opt-out selects); `warnings` lists W013 codes recorded
for the run. Walls are host-time and appear as limits only. `usage.nodes`
is the whole-document arena length for every demand, because
`limits.nodes` (X001) bounds the arena, so `headroom.nodes` stays the
X001 observation; the demanded node count is `cost_model.scope_nodes`.
The remaining `usage` fields follow the tree the demand materialised, so
under a cone `usage.depth`, `usage.array_len`, `usage.keys`,
`usage.value_size`, `usage.total_size` and their `headroom` entries
describe the cone value, while `usage.nodes` and `headroom.nodes` stay
the whole-document observation. `cost_model` below carries the cone's own
node count and charged units.

`cost_model` is the L-P40 three-class demand report, computed from one
linear pass over the topological order that recomputes the same
per-node DP bounds the enforcing budget pass already used (no
enforcement, and no walk of the value tree):

- `demand` — `"all"`, or the cone path when the run was a `Cone(P)`
  (the shape is a string either way).
- `scope_nodes` — the number of arena nodes the demand materialises:
  the whole arena for `"all"`, the closure's member count for a cone.
- `classes` — one `{class, nodes, charged_units}` per class, in the
  fixed order `literal`, `alias`, `computed`. `nodes` counts the
  nodes of that class; `charged_units` is their summed DP size.
  A node's class is the maximum of its own class and its children's:
  `literal` is a decided value, `alias` is a reference hop, `computed`
  is anything evaluated from other nodes.
- `nodes` — the top 256 `{path, node, class, depth, charged_units,
  reason}` by `charged_units` descending, then `path`, then `node`
  ascending; each node's `charged_units` is its DP `size[i]`.
- `truncated` — true when the ranking was cut to 256.

`charge` is what the run that produced the value spent (L-P40 §3.4):
`{total, limit}`, where `total` is the demand-seeded accountant balance
(the closure's DP total plus the measured dynamic growth the walk
charged) and `limit` is the effective `max_total_value_size` ceiling
(`18446744073709551615`, the `UNLIMITED` sentinel, when disabled,
matching `limits.total_size`). It is `null` for a
structural caller that never ran the accountant. Every `profile` body is
a closure member whatever the demanded path, because profile selection
is decided during the walk; a cone that does not reach a body still
charges it, the safe over-approximating direction.

The report is deterministic for a fixed input tuple and never carries
secret material: every `path` and `demand` segment is mapped through the
active profile's redactor, so a material segment renders as the
redaction marker, and a material node is sized at the marker length, so
neither the material's value nor its length appears; `reason` is a
fixed phrase naming the class and, for a `computed` node, its
constructor (`literal`, `alias chain depth N`, `schema shape`,
`container shape`, `computed: env | secret | blob | interpolate |
binary | unary | constraint | ...`).

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

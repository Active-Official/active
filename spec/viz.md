# Visualizer exports (TOOLING §6)

Every emitter returns redacted JSON (`version: 1`). Labels are paths and
kinds; values render through the canonical redactor.

## `active viz <kind> --from <file>`

| Kind | Output | CLI formats |
|---|---|---|
| `graph` | `{nodes: [{id,label}], edges: [{from,to}]}` | `json` (default), `mermaid`, `dot` |
| `taint` | `{version, nodes, edges, sources, export_points}` | `json` |
| `budget` | `{version, mode, limits, usage, headroom, warnings}` | `json` |
| `schema` | INSPECT v2 document plus `mode` | `json` |
| `repair` | `RepairOutcome` (goal, steps, clean/stuck/unrepairable, repaired_source) | `json` |

### Taint

Nodes are every path whose subtree contains a secret; `secret: true`
marks the Secret leaf itself. Edges run containment parent → child
(`""` is the root). `sources` and `export_points` are the secret paths;
no value, locator, or payload is emitted.

### Budget

`limits` is the effective `Budget`; `usage` is the static observation
(nodes = arena length, depth and keys from the evaluated tree, sizes
from canonical redacted JSON); `headroom` is `limit - used` (null when
the limit is disabled with `0`); `warnings` lists W013 codes recorded
for the run. Walls are host-time and appear as limits only.

### Repair

The plan is produced by the shipped engine (`validate` goal, caller's
step cap), so `viz repair` and `active repair` agree by construction.

## Static viewer

`viz/index.html` + `viz/viewer.js` render any export from a file picker:
budget tables, taint graph lists, schema field tables, or step lists.
Zero dependencies, zero network, no evaluation — the JSON is already
redacted.

## Secret guarantee

Tests pin that the serialized taint, budget, schema, and repair exports
never contain fixture secret material, and that the graph emitters
carry names only.

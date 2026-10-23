# Dependency-Graph Tooling (C-049, P-045)

The IR already carries a `petgraph` DAG (`Module.graph`, edges
dependency → dependent). This item exposes it: `ACTIVE graph`,
`ACTIVE impact <key>`, `ACTIVE why <key>`, a library API, and three
language-server methods for agents and dashboards.

## 1. Semantics

- **Nodes** are arena nodes (`NodeId`); **edges** run dependency →
  dependent (evaluation order). Cycles cannot evaluate (S001), but
  queries tolerate them via visited sets.
- **Labels** are dotted value paths (tables win ties, like
  resolution), `let` names for bindings, and `node{id}` for
  unmapped internals. Labels are names only — values (and secrets)
  never appear in graph output.
- **impact \<key\>** = transitive dependents of the key's node
  (what breaks if this changes), excluding the key itself and
  table nodes: a changed leaf trivially rebuilds its parents,
  which would otherwise put every table and the root in every
  answer. Patch and value nodes stay — those are the consumers
  that matter. Sorted by path.
- **why \<key\>** = transitive dependencies (what this value is
  built from), excluding the key itself, sorted by path.
- Key resolution mirrors the evaluator: `resolve_value_path`
  (table fields win ties, single segments also see `let`
  bindings). Array elements address only via their array (no
  index paths). Unknown keys are hard errors naming the key.

## 2. Formats

- **text** (default): the graph prints `Bindings:` with
  `let name = NodeId(n)` lines (alphabetical), then `Graph edges
  (dependency → dependent):` with sorted `a.b → c.d` label lines
  (raw ids remain available in JSON); impact/why print one path
  per line.
- **mermaid**: `flowchart TD` with `id["label"]` nodes and
  `A --> B` edges. Node ids are `n<arena-index>`.
- **dot**: `digraph ACTIVE {` with `"label" -> "label";` edges
  (labels quoted; quotes/backslashes escaped).
- **json**: `{"nodes": [{"id": "n3", "label": "a.b"}], "edges":
  [{"from": "n1", "to": "n3"}]}` (impact/why:
  `{"key": "a.b", "paths": [...]}`).

All outputs sort nodes/edges/paths deterministically: same input,
same bytes.

## 3. CLI

- `ACTIVE graph [--format text|mermaid|dot|json] <file>` (default
  text, the pre-C-049 shape preserved).
- `ACTIVE impact <key> [--format text|json] <file>`.
- `ACTIVE why <key> [--format text|json] <file>`.
- Unknown keys and unparseable files exit non-zero; queries need
  only the parsed module (no evaluation).

## 4. Library API (`active_core::graph`)

`resolve_key`, `transitive_dependents`,
`transitive_dependencies`, `node_label`, `to_mermaid`, `to_dot`,
`to_graph_json`, `impact_paths`, `why_paths` — pure functions over
`&Module`, no I/O.

## 5. Language server

Three custom methods (no capability advertisement needed):

- `active/graph` (`{textDocument}`) →
  `{"mermaid": …, "dot": …, "json": …}`.
- `active/impact` (`{textDocument, position}`) →
  `{"key": …, "paths": […]}` or `null` off-symbol.
- `active/why` (same shape as impact).

Position-based like references: identifiers and interpolation
references initiate; anything else returns `null`. Served from the
open document's module — always in sync with diagnostics.

## 6. Testing contract

- Library unit tests: impact/why on chains, diamonds, and
  isolated keys; unknown keys error; formats validate
  (DOT/Mermaid snapshots, JSON re-parse).
- CLI tests for all three commands incl. formats and exit codes.
- Wire tests for the three LSP methods.

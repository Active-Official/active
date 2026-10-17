# ACTIVE Language Specification (v0.9 — alpha surface)

**Status:** normative companion to `spec/grammar.md` (C-014). This document
defines the abstract syntax, the lowering from concrete syntax to the
intermediate representation, the evaluation model, and the unit system. Where
this document and `spec/grammar.md` disagree with the implementation, that is a
bug — file it, fix the implementation or the spec, never let them drift.

---

## Chapter 1 — Abstract syntax

### 1.1 The module

A parsed ACTIVE source file is a **module**:

```
Module {
    arena: Vec<Node>,                    // all nodes, arena-allocated
    bindings: HashMap<String, NodeId>,   // let name → node
    graph: DiGraph<NodeId, ()>,          // dependency graph
    node_to_graph: HashMap<NodeId, NodeIndex>,
    root: Option<NodeId>,                // the root table node
    node_spans: HashMap<NodeId, Span>,   // source spans for diagnostics
    diagnostics: Vec<Diagnostic>,        // accumulated during parsing
    source_name: String,
}
```

`NodeId` is an index into `arena`. Nodes are immutable once built except during
resolution, where `Ident` nodes are rewritten in place into `Ref` nodes (the id
is stable, so graph edges recorded against the id remain valid).

### 1.2 Node kinds

```
Node = Literal(Value)                         // constant
     | Env { name: String, default: Option<NodeId> }
     | Secret { locator: NodeId }
     | Ref(NodeId)                            // resolved reference
     | Ident(String)                          // unresolved reference
     | Interpolate { parts: Vec<InterpPart> }
     | Binary { op: BinOp, lhs: NodeId, rhs: NodeId }
     | Unary { op: UnOp, expr: NodeId }
     | Array { elements: Vec<NodeId> }
     | Table { fields: IndexMap<String, NodeId> }
     | Quantity { value: NodeId, unit: Unit }
     | ApplyConstraint { target: NodeId, constraint: Constraint }
     | Patch { target: NodeId, ops: Vec<PatchOp> }

InterpPart = Literal(String) | Expr(NodeId)
```

`BinOp = Add | Sub | Mul | Div | Eq | Ne | Lt | Le | Gt | Ge | And | Or`.
`UnOp = Neg | Not`.

### 1.3 Values

```
Value = Null | Bool(bool) | Int(i64) | Float(f64) | String(String)
      | DurationMs(u64)                      // time, canonicalized to ms
      | Quantity(f64, Unit)                  // rates and custom units
      | Array(Vec<Value>) | Table(IndexMap<String, Value>)
      | Secret(String)                       // opaque; redacted on export
```

### 1.4 Diagnostics

```
Diagnostic {
    severity: Error | Warning | Info | Hint,
    code: String,                            // stable, e.g. "E001", "W001"
    message: String,
    path: Option<String>,                    // dotted value path when known
    span: Option<Span>,                      // source position
    repair: Option<Repair>,                  // suggestion + confidence
    confidence: f64,                         // 0.0–1.0
}

Span { line: u32, column: u32, end_line: u32, end_column: u32 }
```

---

## Chapter 2 — Lowering (concrete syntax → IR)

### 2.1 Statement lowering

| Concrete | IR |
|---|---|
| `let n = e` | `e`'s root node registered in `bindings[n]` |
| `[t]` + body | `Node::Table { fields }` inserted into the parent table under key `t` |
| `k = e` (in table) | `e`'s root node inserted into the enclosing table under key `k` |
| `k = e` (at document root) | same, into the root table |
| skipped junk | nothing (no node) |

The **root table** is a synthetic `Node::Table` holding every top-level key.
It is created last and becomes `module.root`.

### 2.2 Expression lowering

| Concrete | IR |
|---|---|
| literal | `Literal(Value)` |
| `name` | `Ident(name)` — rewritten to `Ref` during resolution |
| `env.N` / `env.N ?: d` | `Env { name: N, default }` |
| `secret(x)` | `Secret { locator: x }` |
| `secret.env("N")` | `Secret { locator: Env{name: N} }` |
| `"a${n}b"` | `Interpolate { parts: [Literal("a"), Expr(Ident(n)), Literal("b")] }` |
| `a op b` | `Binary { op, lhs, rhs }` |
| `-x` / `!x` | `Unary { op, expr }` |
| `n unit` | `Quantity { value: n, unit }` |
| `[a, b]` | `Array { elements }` |
| `{ k = v }` | `Table { fields }` |
| `(e)` | `e` (transparent) |

### 2.3 Dependency edges

Edges run **from dependency to dependent** so a topological sort evaluates
dependencies first. `build_graph` records one edge per referenced child:

- `Binary` → edges from `lhs`, `rhs`
- `Unary` → edge from `expr`
- `Interpolate` → edge from each `Expr` part
- `Quantity` → edge from `value`
- `Array` → edge from each element
- `Table` → edge from each field
- `Secret` → edge from `locator`
- `Env` → edge from `default` when present
- `Ref` (resolved from `Ident`) → edge from the referent

`env` nodes, literals, and the root are sources (no incoming edges). Explicit
`depends_on` edges (Phase 1, C-021+) will be added in the same pass.

### 2.4 Name resolution order

For each `Ident(name)` encountered while parsing a scope S:

1. If `name` equals the key currently being defined in S → **skip** (a key's
   own value may not resolve to itself; falls through to step 2).
2. If `name` is another key of S → rewrite to `Ref(that key's node)`, record
   the edge. S includes the enclosing table body, inline table, or document
   root.
3. Otherwise leave as `Ident` for step 4.

After parsing completes, `build_graph` resolves remaining `Ident` nodes against
`bindings` (module-level `let`s), rewriting to `Ref` and recording edges.
Anything still `Ident` at evaluation time is unresolved → W001 + `null`.

Resolution runs after a scope is fully parsed, so **forward references within
a scope resolve**.

### 2.5 Include lowering (C-025)

`include "path" [as alias] [private]` splices another file's module into the
including module, sharing one arena, one binding table, and one dependency
graph:

- The included file is parsed with a fresh scope stack but the **shared**
  bindings, so its references resolve against the combined namespace and
  the includer's `let`s are visible inside it (parameterized includes).
- Its `let` bindings always merge into the shared bindings (last file wins
  on collision goes to the *local* file: an included binding never
  overwrites an existing one).
- Its root-table keys merge by mode: **public** (default) inserts them into
  the enclosing scope's fields as defaults — an explicit local key always
  wins, and among several included files the LAST include wins (cascade).
  Tables merge recursively with local-wins at every level; a scalar from
  a later include replaces only include-provided paths, never a table
  that still holds explicit local keys. `as alias` nests the whole root
  table under `alias` instead. **Private** puts the keys (or the alias)
  into the shared bindings: referenceable, never exported.
- Include cycles (a file already on the include stack) and depth beyond 32
  are E016; the composed graph's value cycles are still S001 as usual.

### 2.6 Profile lowering (C-027)

`profile "name" { ... }` parses its body exactly like a table body
(statements, sibling resolution within the profile, includes) into a
table node stored in `module.profiles` under `name` — never in the root
fields, so profiles never export directly. References from a profile to
document-root keys resolve after the root completes (root fallback),
then to `let` bindings; references the other way (base to profile
content) do not exist. Profile table nodes and their edges join the
dependency graph like any other table, so profiles participate in cycle
detection and evaluation like ordinary values.

---

## Chapter 3 — Evaluation model

### 3.1 Properties (normative)

1. **Deterministic** — same module ⇒ same value tree, same diagnostics. No
   HashMap iteration order, time, randomness, or environment value other than
   the declared `env` reads may influence the result.
2. **Hermetic** — the only external inputs are environment variables read
   through `env.NAME` nodes and files pulled in through `include`
   statements (C-025): literal paths only, confined to the including
   file's directory tree, no network. No shell or process access, ever.
3. **Bounded** — the parser enforces the 64-level expression nesting limit;
   evaluator-side budgets (node count, depth, array length, total keys,
   wall-clock) arrive with C-037.
4. **Observable** — the dependency graph and the diagnostics are first-class
   outputs, queryable by tools.

### 3.2 Algorithm

```
evaluate(module):
    if graph has a cycle:            # is_cyclic_directed, checked FIRST
        return Err(CycleDetected)    # S001 — before any value is produced
    order = toposort(graph)          # dependencies first
    values = {}
    for node_id in order:
        v = eval_node(node_id)       # reads only already-computed values
        values[node_id] = v
    return values[root], diagnostics
```

`eval_node` per kind:

| Node | Behavior |
|---|---|
| `Literal(v)` | `v` |
| `Env { name, default }` | `$name` if set, else the default's value, else `null` |
| `Secret { locator }` | `Secret(locator_string)`; non-string locator ⇒ empty secret |
| `Ref(id)` / `Ident` | the referenced value; unresolved ⇒ W001 + `null` |
| `Interpolate { parts }` | string concatenation; a `Secret` part ⇒ **Err(SecretLeak)** (S004) |
| `Binary` | typed arithmetic/comparison/boolean (3.3) |
| `Unary` | `-` on numerics, `!` on booleans |
| `Array` | element values in order |
| `Table` | field values in `IndexMap` insertion order |
| `Quantity` | normalization per Chapter 4 |

### 3.3 Arithmetic rules

- `Int`/`Float` arithmetic is homogeneous or mixed (mixed promotes to `Float`);
  integer division by zero is a `TypeError` (T001).
- `DurationMs` arithmetic: `+`/`-` with another duration, `*`/`÷` with an
  `Int`/`Float` scalar, `÷` duration-by-duration ⇒ `Int` ratio; ordering
  comparisons on two durations. Adding a duration to a non-duration is a
  `TypeError` — dimensional safety is preserved.
- `Quantity(f64, Unit)`: equality requires equal value **and** equal unit.
- `==`/`!=` are structural for all values (see `value_eq`).
- Any unsupported operator/operand combination is a `TypeError` (T001) with
  the operator and both operand values in the message.

### 3.4 Cycle rejection

Cycles are detected **before** any node is evaluated: `evaluate` returns
`Err(EvalError::CycleDetected)` and the caller never sees a partial value
tree. This invariant is property-tested
(`active-core/tests/proptest_invariants.rs::cycle_is_detected_before_any_value`).

### 3.5 Profile selection and merge (C-027)

After the root table evaluates, selection reads the root value's
`active_profile` field (or a CLI `--profile` override, which wins):

- Missing → no profile merges; the base tree is the result.
- A string naming a defined profile → that profile's evaluated table
  merges into the root value: tables recurse, scalars last-wins
  (profile wins). A profile value whose type differs from the base
  value still wins, with a W003 warning naming the path.
- A string naming nothing → W002, no merge. A non-string → W002, no merge.

Every profile evaluates whether selected or not: an unselected profile's
errors fail the run (strictness catches rot in inactive environments).
Selection reads the BASE `active_profile` only; a profile that sets
`active_profile` itself does not reselect.

---

## Chapter 4 — The unit system

### 4.1 Canonicalization

Units are part of the type. `Quantity { value, unit }` nodes normalize at
evaluation:

| Dimension | Canonical form | Example |
|---|---|---|
| Time | `DurationMs(u64)` milliseconds | `5s` → `5000ms`, `15m` → `900000ms` |
| Bytes | `Int(i64)` bytes | `512MiB` → `536870912`, `100MB` → `100000000` |
| Rates / custom | `Quantity(f64, Unit)` — original unit retained | `100/req/s` → `Quantity(100.0, RequestsPerSecond)` |

Time normalizations: ns/10⁶→ms, µs/10³→ms, s×10³, m×60000, h×3600000,
d×86400000. Byte normalizations: KiB=2¹⁰, MiB=2²⁰, GiB=2³⁰, TiB=2⁴⁰;
KB/MB/GB/TB are decimal (10³ … 10¹²). All results clamp at zero and round to
the nearest integer.

Rates are **not** cross-normalized: `50/m` stays `50 per minute`. Rate units
are dimension-bearing values, not scalars.

### 4.2 Serialization of quantities

- `DurationMs` → JSON `{"ms": N}`; TOML-ish `Nms`.
- `Quantity` → JSON `{"<suffix>": N}` (e.g. `{"req/s": 100.0}`); TOML-ish
  `N<suffix>` (e.g. `100req/s`); interpolation `N<suffix>`.
- `Unit::suffix()` is the canonical spelling and is always re-parseable
  (round-trip property-tested in `active-core/tests/unit_roundtrip.rs`).

### 4.3 Dimensional safety

Operations that would mix dimensions are `TypeError`s, not silent coercions:
`5s + 512MiB` (duration + byte count), `"5" + 1` (string + int). There is no
implicit coercion anywhere in the evaluator. Strings that *look* like numbers
stay strings — safe, explicit coercion is a C-016 healing concern, applied only
under a schema.

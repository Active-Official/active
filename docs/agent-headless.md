# Headless agent flows (redacted, JSON-first)

Copy-paste loops for an autonomous agent. Every command exits nonzero on
a refusal, so shell `&&` chains stop where they should. Secrets never
appear in any output below; redaction is upstream of the shapes.

## 1. Check, then inspect the failure

```sh
active check svc.active || active explain --from svc.active --format json
```

```sh
active inspect --from svc.active --path server.port --format json
```

Read `schema.constraints`, `schema.description`, `dependencies`, and
`content_addressed` instead of re-parsing source text.

## 2. Plan, then apply a subset

```sh
active repair --from svc.active --browser > plan.json
active repair --from svc.active --apply-step 0 --output step1.active
active repair --from svc.active --cherry-pick server --output server-fixed.active
```

`plan.json` has `diagnostics` + `plan.steps` (kind, path, directive,
safe, status, side_effects). Apply only safe steps; `stuck` (H006) and
`unrepairable` (H008) are terminal signals.

## 3. Step through a failure

```sh
active debug --from svc.active --break code:T003 --format json
active debug --from svc.active --minimize T003 --format json
```

The breakpoint output stops at the first node that emitted the code; the
minimized case is a line-subsequence that still fails with the same
code, ready to attach to a regression.

## 4. Budget and mode

```sh
active mode --from svc.active --format json
active budget explain --from svc.active --format json
active viz budget --from svc.active > budget.json
```

`mode.capabilities` is the declared set; `budget.headroom` says how much
room remains per limit. Data-oriented constructs without
`active_mode = "data"` trip lint rule L010.

## 5. Visual data for a dashboard

```sh
active viz graph  --from svc.active --format json > graph.json
active viz taint  --from svc.active > taint.json
active viz schema --from svc.active > schema.json
active viz repair --from svc.active > repair.json
```

The four shapes with committed schemas (graph, taint, budget, mode) are
pinned in `docs/schemas/`; `viz/index.html` renders them with no network
access. Tool JSON without a committed schema is covered by CLI tests.

## 6. Editor-free verification

```sh
active lint --format sarif svc.active > active.sarif
active capability --require-major 1 > descriptor.json
active compatibility old.active new.active --format json
```

SARIF 2.1.0 uploads to code scanning; the descriptor handshake fails
closed on a major mismatch; compatibility exits nonzero on breaking
changes.

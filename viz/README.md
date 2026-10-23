# Active visualizer (static)

Open `index.html` in a browser (file:// is fine) and load a JSON export
produced by:

```sh
active viz graph --from doc.active --format json    > graph.json
active viz taint --from doc.active                  > taint.json
active viz budget --from doc.active                 > budget.json
active viz schema --from doc.active                 > schema.json
active viz repair --from doc.active                 > repair.json
```

The page renders budget tables, taint nodes/edges, schema field tables,
or repair/step lists. It performs no evaluation, makes no network
request, and loads no external assets. Exports are already redacted;
never hand-edit a secret into one.

Shape reference: `spec/viz.md`.

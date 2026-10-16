# CI pipelines

Stages as tables, build matrices as arrays of tables, shared values
via `let` (referenced, not YAML-anchor-aliased without provenance).
`solid check` validates the whole pipeline statically, and `solid
eval` renders the matrix for runners to consume:

```solid
{{#include ./ci-pipeline.solid}}
```

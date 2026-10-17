# CI pipelines

Stages as tables, build matrices as arrays of tables, shared values
via `let` (referenced, not YAML-anchor-aliased without provenance).
`ACTIVE check` validates the whole pipeline statically, and `ACTIVE
eval` renders the matrix for runners to consume:

```active
{{#include ./ci-pipeline.active}}
```

# From YAML

The reason to move: YAML's risks are structural — significant
indentation that silently reshapes, implicit typing (`yes` is a
boolean in 1.1), anchors that alias without provenance. SOLID keeps
the readability and removes the traps: indentation is validated
(`E010` on ambiguity), types are explicit, references are
dependency edges with cycle rejection.

YAML in:

```yaml
server:
  host: localhost
  port: 8080
```

SOLID out:

```solid
{{#include ./yaml.solid}}
```

`solid import --from yaml` converts existing files (best-effort,
with diagnostics for lossy corners). Multi-line strings use
`"""` blocks with common-indent stripping instead of `|`/`>` modes;
keep keys unique — a repeated key silently keeps the last value
(YAML implementations vary here; SOLID picks last-wins
deterministically).

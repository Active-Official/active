# Active vs YAML, TOML, and JSON

An honest comparison by failure class, not feature count. Every claim below
is demonstrated in the migration kits or the playground.

## YAML: the folds

YAML 1.1 reads `NO`, `yes`, `on`, and `off` as booleans (verified against
PyYAML in Kit A). Anchors merge silently; tab indentation is forbidden but
fails far from the cause. Active keeps `NO` a string with W001, has no
anchors to bomb, and rejects tabs at the exact column with E006.

## TOML: the ceilings

TOML has no null, no units, no schema, and no secrets story. Datetimes are
values Active deliberately keeps as strings with W008 (no datetime type by
design). Dotted keys collide with tables at a distance; Active has no
dotted keys at all (E002), so the collision class does not exist.

## JSON: the silence

JSON accepts everything with a shape and explains nothing: no comments, no
trailing commas (rejected without guidance), no schema, numbers without
meaning (`30` vs `30s` vs `"30s"` are three different facts Active keeps
apart). Active adds comments, spans on every diagnostic, units, and closed
schemas.

## What Active refuses that all three allow

Silent coercion (Norway class), unitless quantities where units matter,
cleartext secrets through exports, duplicate keys without warning (W011),
and unbounded evaluation without budgets. The three migration kits turn
each refusal into a worked before/after.

## Token cost, measured honestly

`tools/token-measure.py` counts tiktoken cl100k_base tokens over the Kit
A/B/C documents plus hand-written JSON/YAML equivalents
(`tools/token-corpus/`), measured 2026-10-19:

| Case | Active file | JSON | YAML |
|---|---|---|---|
| service | 185 | 46 | 32 |
| flags | 122 | 71 | 48 |
| agent | 178 | 60 | 45 |
| TOTAL | 485 | 177 | 125 |

Active files cost more because they carry the schema, the comments, and
the env fallbacks: the guarantees made visible. Evaluated payloads sit at
parity (service 55, flags 32, agent 63 tokens of JSON out). Readability
wins ties; these numbers inform, never drive design.

## When not to use Active

Runtimes that only need a final value tree should consume pre-evaluated
JSON (`active eval` output) instead of embedding the evaluator. Documents
that must round-trip byte-identically through YAML tooling should stay
YAML. Active is a source language with opinions, not a universal container.

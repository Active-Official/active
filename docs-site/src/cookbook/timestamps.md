# Timestamps the Active way (L-020)

Active has no datetime type, by design: instants smuggle calendars,
zones, and arithmetic that configs rarely need and often get wrong.
TOML/YAML datetimes import as RFC 3339 strings with W008, and the
blessed pattern below keeps them honest.

## The pattern

```active
schema = { at = String(pattern = "^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$") }
at = "2026-10-19T12:00:00Z"
```

`active check` passes. Write `"tomorrow"` instead and the schema
refuses with `T003: schema violation at 'at': does not match pattern`
at the exact span. Constraints do the validation a datetime type would
promise, without the type system paying for calendars.

## Ordering and arithmetic

Same-format UTC stamps order lexicographically (`"2024-..."` sorts
before `"2025-..."`), which covers expiry and rotation checks. Keep one
format per field (document it in `@intent`); mixed formats do not order.
Durations stay first-class (`30s`, `5m`) for everything time math
touches: deadlines are durations after a stamped start, never instant
arithmetic.

## Why not a type

A datetime type would need zones, daylight rules, parsing leniency
choices, and ordering semantics: an entire calendar smuggled into a
config language whose promise is explicitness. Strings plus patterns
keep the complexity where it belongs (one visible regex) instead of
inside every value.

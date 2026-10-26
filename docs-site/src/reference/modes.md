# Evaluation modes: Config and Data

Active runs every document in one of two modes. The document chooses:
Config Mode is the default, and Data Mode is explicit opt-in.

| | Config Mode (default) | Data Mode |
|---|---|---|
| Marker | none | `active_mode = "data"` at the root |
| Purpose | configure software | store, replay, or exchange data |
| Budget warnings | hard failure at the ceiling | progressive warning from 80%, hard ceiling still binds |
| Data constructs | lint `L010` warning | expected |
| Content addressing | rejected as a mode mismatch | `blob(media_type, data)` |
| Schema evolution | metadata is data | `version`, `min_reader`, `min_writer`, `removed_in` |
| Capability declarations | declaration is data | `active_capabilities = [...]` |

"What counts as a data construct" is deliberately narrow: a
content-addressed value, schema evolution metadata, or a capability
declaration. Everything else - tables, arrays, units, references,
secrets, profiles, patches - behaves identically in both modes.

## Declaring Data Mode

```active
active_mode = "data"
active_capabilities = ["check", "eval", "graph"]

model = {
  version = "1.0.0",
  payload = blob("application/json", "{\"nodes\":3}")
}
```

The marker is a normal root key, so the document stays self-describing:
it travels with the data and an agent can read the mode without
side-channel metadata. A nested key named `active_mode`, or a marker
merged from an include, does not switch the mode; only the root
document's own declaration does.

A malformed marker is `E024`: `active_mode` must be the string `"data"`
or `"config"`. Unknown values fail closed rather than defaulting
silently.

## Mode-sensitive surfaces

- **Budgets.** Config Mode reports a budget failure at the hard
  ceiling. Data Mode emits `W013` once a limit reaches 80% and continues
  until the ceiling. The ceiling never moves; the warning arrives
  earlier.
- **Lint.** `L010 data-mode` reports a data construct in a Config
  document. The fix is either the marker or a different representation;
  the rule never rewrites the document.
- **Capabilities.** In Data Mode a document can declare what it expects
  the host to offer. `active capability --check-doc <file>` performs the
  handshake and refuses unknown entries before evaluation.
- **Memoization.** The incremental memo is keyed by mode, so switching
  a document's mode never reuses an evaluation from the other mode.

## Inspecting the resolved mode

```sh
active mode --from service.active          # text
active mode --from service.active --format json
```

The text form names the mode and the marker; the JSON form is stable and
carries `mode`, `marker` (whether the root document declared it), and
`capabilities`.

The `schema inspect` document also reports `mode`, because a schema
contract and the document that uses it must agree about which mode they
describe.

## Why two modes instead of two languages

One parser, one validator, one budget accountant, and one secret-taint
model mean a value cannot mean one thing in a config file and another
thing in a stored payload. The mode changes which constructs are
expected and how budgets warn; it never changes evaluation order,
redaction, or the meaning of a table.

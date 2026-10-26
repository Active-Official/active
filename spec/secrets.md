# ACTIVE Secrets & Taint Tracking (C-035/C-036)

Secrets are first-class values with a transitive taint model, refusal
points, and audited redaction (P-032). The invariant is absolute:
**evaluated secret material never reaches a diagnostic, a log, an
error message, or an export** — only redaction markers do.

## 1. Taint model

- Taint is the `Secret` value kind plus structural containment:
  composites *contain* tainted values — a table with one secret does
  not taint its siblings (no sideways contagion: non-secret siblings
  stay fully visible and usable). There is no separate taint bit —
  the kind IS the taint, so it cannot be dropped by accident.
- Origins: `secret("literal")`, `secret(<string expression>)`, and
  `secret.env(NAME)` (environment-sourced). A non-string locator
  yields an empty secret — still `Secret`-kinded, still redacted.
- `let` bindings, references, table construction, array
  construction, profile merges, patch applications, and default
  fills propagate secrets transparently (no special casing: the
  value KIND flows with the value).

## 2. Propagation and refusal per construct

| Construct | Rule |
|---|---|
| Interpolation `"…${secret}…"` | **Refused** (S004, fatal): secrets never enter strings. Stronger than tainting — no tainted string ever exists to leak later |
| Arithmetic `+ - * /`, ordering `< <= > >=`, negation `!`/`-` | **Prohibited** (T001): the message names operand *kinds* only, so no operand text (secret or otherwise) can be carried |
| Equality `==` / `!=` | Allowed, pure: secrets never equal anything, **including themselves** (`value_eq` has no `Secret` arm). No oracle — a guess can never be confirmed |
| Tables / arrays | Structural containment: secrets ride inside composites and redact recursively on every export path |
| Profiles / patches / fills / defaults | Structural: merged, patched-in, and filled secrets stay `Secret`-kinded end to end. A profile override that replaces an existing material value inherits its materiality; a profile value at a path no explicit value or fill covers answers for itself (a material expression is redacted, a benign literal stays visible). Only the selected profile contributes (every profile contributes only when the selector is computed, and none when no profile runs). An explicit profile override does not discharge the computed-selector union: the evaluator can still resolve and apply the document's own selector, so selecting another profile cannot declassify its value. A material value seeds copies elsewhere, unless the same node is still exported through a benign path after every profile and patch transform; a computed selector drops every path a possible profile could override, so no unselected profile can hide a seed. Whole-value matching redacts at any length; substring spreading is limited to values of four bytes or more. Resolution follows `let` aliases with cycle detection under fixed node, byte, and depth budgets; exceeding a budget fails the whole analysis closed (every literal material) |
| Resilient H005 renames | Refused at a material source path: moving the value would detach it from the taint map. The typo stays a suggestion-only T005 |
| Canceled fills | A material schema default that a resilient rename preempts may leave the healed key conservatively marked material. The healed value is hidden, never exposed; the conservative direction is deliberate |
| `@constraint` predicates | May read secrets (equality tests, boolean logic); violation rendering redacts actuals to `secret` and secret-bearing sources to generic (spec/annotations.md §6) |
| Schema `min`/`max`/bounds | Readable as values; never echoed (spec/validation.md §7) |
| `Bytes` / units | Orthogonal: byte-ness is erased at evaluation and has no taint interaction |

## 3. Export: redact, never emit

- JSON and TOML converters replace every secret, at any depth, with
  `"***SECRET***"` — the marker, never the data. "Refuse to emit
  tainted data" (P-032) is satisfied by redaction: secret bytes never
  leave the process through an export path.
- `ACTIVE graph` prints node ids and edges only — no values at all.
- `Value`'s `Debug` and `serde::Serialize` impls redact by
  construction (`Secret(***)` / `"***SECRET***"` at every depth), so
  generic logging, assertion output, and direct serialization can
  never emit secret material. Deserialization stays derived: the
  input direction cannot leak.
- `ACTIVE fmt` re-emits `secret(…)` source calls (source-derived
  locators, never evaluated values).
- Debug/error rendering is value-free for type errors: T001 and quantity
  errors name operand kinds (`secret`, `number`, `string`, …), never
  values or markers. Structural material assembled from a plain locator
  therefore has no path into a message.
- No unlock API exists in this phase: there is no audited path that
  emits raw secrets. (Deferred to enterprise work, with P-032's
  explicit-audit requirement noted.)

## 4. Diagnostics and log audit (C-036)

Every message constructor was audited for evaluated-value echo:

- E0xx (parser): names, paths, and spans only — no evaluated values
  exist yet at parse time, and string payloads (schema versions,
  `active_mode` values) are never echoed, so a parse error cannot
  publish a secret.
- W001–W004: identifier names, profile names, key paths — all
  source-authored, never values.
- T001: operand kind names only (value-free: no content, no marker).
- T002–T005: kinds and schema-authored literals only
  (spec/validation.md §7). T006 renders the evaluated actual through
  the structural redactor: a material actual (any kind) becomes the
  marker, and a string containing a known secret is redacted in
  place. W005 (default fills) carries the key path only — filled
  values are never quoted.
- S001/S002/S003: structural messages, no values. S004 carries no
  positions by design (positions could point at secret data).
- E019: patch and annotation paths only.
- H0xx (healer): repairs quote **source text** (tokens being
  repaired), never evaluated values. H005 suggests key names
  (source identifiers, value-free). Caveat: a literal secret written
  in source (e.g. `secret("hunter2")`) appears in its own repair
  record — the same exposure as the file itself, not a new leak.
  Entropy-based log redaction is future work.
- CLI `report`/`report_eval_error`: print diagnostics only — safe by
  the above. Exit codes and file/span prefixes carry no values.

## 5. Anti-oracle rule

Secret equality is always false, even `s == s`. Rationale: equality
against a secret is a guessing oracle; a configuration language has
no legitimate need to compare secret contents (compare locators and
rotation metadata instead, as plain data).

## 6. Determinism

Redaction is a pure function of the value tree: same input always
redacts to the same markers in the same positions.

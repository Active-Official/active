# Debugger: steps, breakpoints, replay, reduction (TOOLING §5)

The debugger is a deterministic observer over the shipped evaluator.
It never re-implements evaluation and never reveals secret material.

## 1. Step trace (DEBUG.001)

`Engine::evaluate_with_steps(&module)` returns the same value and
diagnostics as `evaluate`, plus one `StepEvent` per arena node in
topological order:

```
StepEvent { node: u32, path: Option<String>, kind: String,
            tainted: bool, value: JSON, codes: [String] }
```

- `path` is the dotted value path when the node maps to one.
- `value` renders through the canonical redacting JSON mapping.
- `codes` lists diagnostics emitted while that node was evaluated.
- **Locator redaction:** every node inside a `secret(...)` locator
  subtree renders as `"***SECRET***"` with `tainted: true`. A literal
  locator is not Secret-kinded, so this is enforced structurally, not
  by the value kind. A container keeps its shape and each material
  leaf (string, number, …) renders the marker.
- **Complete sensitive set:** steps render after the walk, once every
  authored literal and runtime secret is known, so evaluation order
  cannot leak a copy into an earlier step. Any step whose render
  redacts a string or drops a blob payload reports `tainted: true`, so
  the `taint` breakpoint and the value never disagree.
- **Transforms run after the walk.** Profile merges, `@patch`
  directives, and schema-default fills apply to the final tree, so
  the root step shows the walked document; `eval` shows the
  post-transform tree. Inspect and hover resolve transform paths
  explicitly (the structural profile maps Set/Remove/Rename and
  schema defaults). A computed `active_profile` selector cannot be
  resolved statically, so the taint check consults a union of every
  profile and fails closed.

Determinism: same source, environment, profile, mode, and budget →
byte-identical step arrays.

## 2. Breakpoints (DEBUG.003)

Selectors match the first step:

| Selector | Matches |
|---|---|
| `taint` | first step whose value contains a secret |
| `path:<prefix>` | first step whose dotted path starts with the prefix |
| `code:<CODE>` | first step whose diagnostic codes include the code |
| `node:<n>` | the step at topological index `n` |

The CLI prints steps through the match (or nothing and exits 1 when a
selector never matches).

## 3. Replay (DEBUG.004)

`debug::replay` compares the redacted content hash, the ordered
diagnostic codes, and the full step arrays. A wall-bounded preflight
runs with the same identity-only blobs as `eval` (no step collection),
so a document that evaluated successfully cannot fail replay on the
heavier canonical work; two canonical materialised step passes then run
without a wall deadline and provide the hash, codes, and steps.
`--expect sha256:...` additionally pins the hash (the audit trail's
`result` value). Any mismatch exits non-zero. Replay hashes the
redacted, payload-free tree, so `--materialise-blobs` and
payload-resident secrets cannot change the identity. The budget and
mode are inputs: an audit produced under custom limits, `--resilient`,
or `--allow-parent-escapes` replays only when `active debug` receives
the same flags. Count and size limits still bind, so an untrusted
document fails fast. Steps are identity-only: payload bytes never enter
a `StepEvent`. A strict validation failure is not replayable: the audit
records `result = null` and `outcome = "failed"`, while replay returns
the validation error; compare the recorded code list instead.

## 4. Minimal failing case (DEBUG.005)

`debug::reduce(source, code, max_attempts)` deletes lines one at a time
while the target diagnostic code survives, through the real
parse/evaluate path. Lines that participate in a `secret(...)` call are
never deleted, so reduction cannot strand a secret literal outside a
redactable call. With a file context the reducer resolves includes, so
a code that lives in an included file is reproducible; signed reduction
(`--trust-keys` with `--minimize`) is rejected rather than silently
ignored. The result is always a subsequence of the original lines; the
attempt count is bounded and reported.

The bundle also reports `target_present` (whether the original source
emits the code at all); the CLI exits 1 when it is false, so an absent
target is never reported as a successful reduction. `debug::minimal_case`
wraps it with sizes for a regression bundle.

## 5. CLI

```
active debug --from <file> [--break SELECTOR] [--replay [--expect HASH]]
             [--minimize CODE] [--max-attempts N] [--format json|text]
             [--profile NAME] [--resilient] [--allow-parent-escapes]
             [--budget-profile P | --max-nodes N --max-depth N ...]
```

Default action prints the step trace. JSON output wraps
`{breakpoint, matched, total_steps, steps}`; text output is one line per
step, then the breakpoint verdict.

Flag combinations are constrained: `--expect` requires `--replay`;
`--break` conflicts with `--replay` and `--minimize`; `--replay`
conflicts with `--minimize`; `--max-attempts` requires `--minimize`.
An invalid combination is a usage error (exit 2 for clap conflicts,
exit 1 for the runtime-checked `--trust-keys`/`--minimize` pair),
never a silently ignored flag. `--minimize` honors the budget,
`--profile`, `--resilient`, and `--allow-parent-escapes` inputs: the
reducer parses and evaluates candidates under the same mode, options,
profile, and count/size limits as the run being explained (it disables
the wall clock for candidates unless `X002` is the target, so reduction
is deterministic).

## 6. Secret guarantee

No step, breakpoint, replay report, or reduction bundle contains raw
secret material: the value mapping redacts Secret values, locator
subtrees are replaced structurally, and `--minimize` rewrites authored
secret literals to `"***SECRET***"`. A duplicate of a known secret
value elsewhere — another string token, a blob payload, a comment, or a
base64/hex encoding of the value — is redacted too. Matching uses exact
decoded token equality (so a short secret does not rewrite unrelated
strings) and substring matching for comments (which are non-semantic);
canonical `secret.env("NAME")` arguments are exempt. Padded, unpadded,
and URL-safe base64 plus hex (with optional `0x` and whitespace) are
decoded when they form the whole value; an encoded secret embedded in a
longer string is not decoded. Multi-line strings are dedented and
escaped dollars unmarked before matching; a nested interpolation that
hides the value is covered when its literal token matches. Taint is structural, so a plain
value used as a locator (a `let`, a table field, an environment lookup)
and any interpolation, reference, or container built from it are
material even without a `Value::Secret` marker; containers keep their
shape and every material leaf carries the marker, whatever its kind
(a number used as a locator is material too). Known secret substrings
are redacted longest-first, and an existing marker is opaque, so a
secret whose text overlaps the marker cannot rewrite it. A line break
between `secret` and `(` (which the parser rejects) is normalized for
redaction only, so malformed calls still redact. The `secret` keyword
matches case-insensitively, so `Secret(` and `secret.Env(` are still
calls. `secret.env(NAME)` names are metadata and survive; later
arguments (such as a default) are redacted. If a call has any shape
other than `secret(` or `secret.env(` — stray dots beside `env`,
`secret env(`, a dotted identifier run, a case variant — or an env
call carries anything past its name, the redactor fails closed by
redacting every string literal in the document except the canonical
`secret.env("NAME")` names, and the reducer protects those call lines.
Regression tests pin the locator, let/env-backed, interpolation,
tainted-blob, newline, table-ref, reduction-stranding, and
malformed-punctuation cases.

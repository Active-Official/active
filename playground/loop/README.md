# Playground repair loop (GAP-CLOSURE §2.10)

The playground evaluates documents. This directory demonstrates the full
agent loop on a real, shipped path:

```text
generate (JSON Schema)  ->  check (fails)  ->  repair plan
  ->  partial apply (one step)  ->  eval (still fails)
  ->  full repair  ->  eval (clean)
```

Everything below is executed by `run.ps1` (or `run.sh`) against the
current `active` binary. `cargo test -p active-cli --test loop_demo`
re-runs the same pipeline and asserts each verdict, so the transcript
cannot drift from the implementation.

## 1. The broken document

`broken.active` declares a closed schema with three long keys and
supplies two of them with near-identical typos
(`application_database_prt`, `application_database_hots`). The loop sees
two `T005` unexpected-key errors and two `T004` missing-required errors:
the same defect from both directions.

## 2. Generate the contract

```sh
active generate --from broken.active --to jsonschema
```

The schema block becomes a JSON Schema document — the artifact an agent
hands to a validator or another service.

## 3. Check

```sh
active check broken.active
```

Exit code 1 with stable `T005` / `T004` codes and paths. The codes and
paths are the machine-readable contract; messages are for humans.

## 4. Plan the repair

```sh
active repair --from broken.active --format json
```

The plan lists the safe `@rename` directives (both typos clear the
auto-apply confidence threshold). `clean: false` means the original
document still fails until a step is applied.

## 5. Apply one step (partial repair)

```sh
active repair --from broken.active --apply-step 0 --format json \
  --output partial.active
```

Only the selected step is applied. The JSON `repaired_source` is byte
equal to `partial.active`, unselected steps stay `skipped`, and the
reported verdict describes that subset: one missing key remains, so
`eval partial.active` still fails. Partial repair tells the truth about
what it wrote.

## 6. Apply the full plan and evaluate

```sh
active repair --from broken.active --output repaired.active
active eval repaired.active
```

The second run applies both renames; the output is clean and `eval`
prints the repaired tree. A third repair run reports `clean: true` with
no steps: the loop is idempotent, which is what makes it safe for an
agent to run repeatedly.

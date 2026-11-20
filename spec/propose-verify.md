# Propose and gate: the verifier as product (L-P62)

The generator does not need to be trusted, because the verifier is the
gate. `active gate` verifies one candidate; `active propose` runs the
propose → verify → (accept | repair and re-verify) loop. There is no
model in tree.

`active verify` is the signature-verification command; the candidate
verifier is named `gate` so the two never collide. The vocabulary
(propose, verify, accept, reject) stays the loop's.

## 1. `active gate <candidate>`

One candidate, one verdict. The checks are the shipped per-file
conformance checks, in this order, and they are the same stages
`active test` runs (one implementation, one verdict):

1. **parse** — strict, include-aware; any parse diagnostic rejects.
2. **healing** — a document the parser had to repair rejects
   (`spec/formatter.md` section 2: a repair is never baked into a file).
3. **format** — the source must equal its canonical indentation-style
   form (`spec/formatter.md`); a formatter refusal rejects too.
4. **evaluation** — strict engine under the caller's budget, profile,
   mode, environment, and reader version; schema and annotation
   validation run, and error severities reject (warnings reject only
   under `--strict`).
5. **lint** — the project's discovered `.active-lint.toml` applies
   exactly as `active lint` applies it.

A finding carries its stage, severity, stable codes, and a value-free
detail. The verdict is `accept` when no fatal finding exists, else
`reject`; exit 0 or 1. Diagnostics and verdicts never echo values or
material text (the redaction rules of `spec/audit-trail.md` and
`spec/secrets.md` apply unchanged).

`gate` takes the evaluation inputs `eval` takes where they affect the
verdict: `--profile`, the budget flags, `--reader-version`, `--env`,
`--allow-parent-escapes`, `--trust-keys`, and `--lockfile` (defaulting to
a sibling `active-lock.toml` when trust is set, exactly like `eval`). A
lockfile pin that no longer matches rejects (E041), so the verifier
cannot accept a document `eval` refuses.

`--receipt` writes atomically through a fresh sibling file and refuses to
replace an existing path unless `--force` (a symlink at the path is
replaced as a link, never followed).

`--format json` prints `{verdict, file, findings, result}`; `result` is
the bound content hash of an accepted candidate's redacted value and
`null` on rejection.

### Receipt

`--receipt <path>` writes an L-T06 envelope v2 (`spec/tool-result-envelope.md`):
`result` binds payload + schema version + profile + resolved budget + mode
+ capability; `payload` is the redacted evaluated tree on acceptance and
`null` on rejection; the receipt records `outcome` (`clean`/`failed`),
`failure_class` (`parse` for parse or size failures, `validation` for
schema/annotation failures, `other` for format/lint), capability, mode,
budget, schema version, and the diagnostic codes. A rejection that never
parsed still gets a receipt (failed attempts carry one).

## 2. `active propose`

```
active propose --request <text|@file>
              (--candidate <path|-> | --generator <command>)
              [--rounds N] [--output <path>] [--receipt <path>]
              [gate options]
```

- **Generator.** The first generator is the caller: `--candidate` takes
  the candidate as-is (`-` reads stdin), which is the "paste the LLM
  output" path. `--generator <command>` runs the command with the prose
  request on stdin and takes its stdout as the candidate; a non-zero exit
  is a generation failure (exit 1), never a rejection. The command runs
  through `sh -c` on Unix and through an exclusively-created temporary
  `.cmd` file on Windows. It has a 60-second deadline, its output is
  capped at 8 MiB (fails closed above it), and its streams are drained so
  a chatty or stdin-ignoring generator cannot deadlock the tool. No
  network, model, or secret is part of the contract.
- **Working copy.** Rounds verify the candidate text directly (no path
  read, so nothing can swap what is checked). The card is a hidden,
  exclusively-created file in the candidate's directory (0600 on Unix;
  the formatter/lint/include context is unchanged) and is removed on
  every exit path. `active test` walks skip hidden files, so a concurrent
  run cannot see it.
- **Rounds.** `--rounds N` defaults to 2 and is capped at 5. Round 1
  verifies the candidate. On rejection, the shipped repair loop
  (`spec/repair.md`) plans directives against the diagnostics; when it
  produces a changed source, the formatter normalizes it and the next
  round verifies again. A round that finds no safe step stops the loop.
- **Bound.** Exhaustion fails closed with `H008` naming the remaining
  checks or codes; a bounded loop never silently accepts.
- **Output.** `--output` is written only when a round accepted the
  candidate; a rejected candidate is never written. `--output` and
  `--receipt` are written atomically and refuse to replace an existing
  path unless `--force`. A warning explains an omitted output.
- **Record.** `--format json` prints `{verdict, rounds, terminal, result,
  output}`; each round carries its findings. `--receipt` writes the final
  L-T06 receipt with the same shape as `gate`. A candidate that never
  parsed has no module to bind: its receipt records `outcome: failed`,
  the failure class, and the codes, without a mode/budget/schema binding
  (there is nothing to bind).

## 3. Verdict parity

`gate`'s evaluation findings must name the same blocking codes `eval`
refuses on for the same input and options (one language, one verdict).
Format and lint are gate-only by design. The differential test in
`active-cli/tests/cli.rs` pins the evaluation parity, and
`conformance/lp62/` pins the command verdicts with the shipped binary.

## 4. Non-goals

No generator is shipped; no rejected candidate is written; no new
diagnostic code is introduced; `eval`'s verdict semantics do not change.

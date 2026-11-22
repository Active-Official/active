# Active reference interpreter (L-P37)

A second, independent implementation of Active's core semantics in
Python, plus a differential runner against the shipped Rust engine.
This is a test oracle, not a product: it is not a workspace member, is
not published, and is never imported by a shipped crate.

Authority: the approved design at `audit-pack/L-P37-DESIGN.md` (sha256
`810d1fa8f238597634ef582a2e3b25eea04f8021274f80985b2fd0ac2dd00e1e`)
and Bud's normative gap adjudication at
`audit-pack/BUD-GAP-DECISIONS-L-P37.md`. Where the design leaves a spec
question open, Bud's decision wins; a new question is filed as a gap,
not guessed.

## Status

Implemented and green at the L-P37 commit: **69/69 corpus cases and
300/300 Monte Carlo documents, zero divergences** (the corpus reference
is the committed `expected/`/`errors/` sidecars; MC uses the Rust
sweep's deterministic dump). Two consecutive runs produce byte-identical
reports. The package is written from the spec chapters listed in the
design §3.1, never from `active-core/src` while a behaviour is being
implemented.

The differential compares outcome, redacted JSON values, ordered
diagnostic records `(code, severity, path, span)`, duplicate counts,
`content_hash`, and scans every output for material: the fixed fixture
markers plus every runtime seed the oracle collected (exact matches at
any length, substrings at four bytes). The corpus reference is the
`ACTIVE_CORPUS_DUMP` projection from the Rust conformance test; the
committed sidecars remain the fallback when no dump is supplied (they
are a deduplicated code summary). Two runs produce byte-identical
reports. A case-level exclusion needs a gap id in `known-gaps.toml`; the
current file is empty because every case agrees.

Known scope limits: include resolution has a descriptor-based
containment check (O_NOFOLLOW plus `/proc/self/fd` on Linux; Windows
relies on the textual check), a dotted reference through an aliased
include is unresolved (matching the shipped parser), and the oracle's
include composition covers schema, patches, annotations, evolution
metadata, and `private`. Envelope v2 checks receipt/sender/result shape
and bound-result equality after transforms; `@constraint` T006 and the
schema/include/profile salt projections are implemented. Remaining
residuals are filed in `audit-pack/BLOCKS-L-P37.md` (sender signature
verification, T006 cap, receipt-budget equality, exact nested patch
accounting, wall clock).

## Layout

```
active_ref/
  diag.py        Diagnostic records, severity, stable codes
  tokens.py      Token kinds and spans
  lexer.py       UTF-8 lexer, both layouts, units, strings/interpolation
  model.py       Module/Node/Value dataclasses (the spec's IR)
  parser.py      Statements, expressions, schemas, profiles, patches
  units.py       Unit table, canonicalization, dimensional rules
  eval.py        Evaluation, names, profiles, patches, budgets, taint
  validate.py    Schema validation, defaults, checks, closed tables
  export.py      Redacted canonical JSON bytes and content hashes
  includes.py    Include resolution, pins, merge order
  api.py         parse/evaluate entry points for the runner
diff.py          Differential runner over the manifest + MC dump
tests/           Unit tests for the oracle itself
```

## Running

```
python -m pip install -r tools/reference-interpreter/requirements.txt
python -m unittest discover -s tools/reference-interpreter/tests
ACTIVE_CORPUS_DUMP="$TMP/corpus.jsonl" cargo test -p active-core \
    --test conformance l37_corpus_dump
ACTIVE_MC_DUMP="$TMP/mc.jsonl" cargo test -p active-core --test mc_grammar \
    monte_carlo_grammar_sweep
python tools/reference-interpreter/diff.py --corpus-dump "$TMP/corpus.jsonl" \
    --mc-dump "$TMP/mc.jsonl"
```

The design named `active eval --envelope` as the reference, but the CLI
envelope is Data-mode-only; the committed sidecars are the stable
reference for the 69 Config/Data cases (the conformance test already
proves the engine matches them), and the MC dump is the generated
reference. This is recorded in `audit-pack/L-P37-DIVERGENCES.md`.

## Independence rules

- Write from `spec/`; do not read `active-core/src` while implementing a
  behaviour.
- Every divergence record carries a spec citation or a gap id.
- Never weaken a property, narrow an input set, or edit the committed
  conformance sidecars to match the oracle.

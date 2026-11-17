# Interop vectors (LANG-E-002)

Owned contract between this workspace's text layer and the published binary
codec (`active-infinity`). The pairing record is
[`docs/interop-pairing.md`](../../docs/interop-pairing.md); this directory is
the executable half.

## Tiers

| Directory | Runs where | Contract |
|---|---|---|
| `cases/` | text test + codec checker | the published codec parses the document, round-trips it through `encode`/`decode`, and projects a value semantically equal to this workspace's export |
| `structural/` | text test + codec checker (round-trip only) | expression graphs survive the codec structurally and canonically; the codec's JSON projection renders expression nodes as markers |
| `current/` | text test | kinds that need a newer `active-core` than the published 1.4.0 the pinned codec depends on; joins `cases/` at the next re-pair |

Each tier has an expectation directory (`expected/`,
`expected-structural/`, `expected-current/`) holding the exact redacted JSON
this workspace exports. Paired cases additionally have
`expected-infinity/`, the pinned codec's redacted projection.

## Commands

```sh
# Text side: the shipped parse -> evaluate -> profile-redact path
cargo test -p active-core --test interop_vectors

# Codec side: the real published codec, pinned by checker/Cargo.toml
cargo run --locked --manifest-path conformance/interop/checker/Cargo.toml

# Pairing record and pin lockstep
cargo test -p active-core --test docs_contract
```

The checker is its own workspace, so `cargo test --workspace` never builds
the crates.io dependency. CI runs it in the Conformance workflow's `interop`
job.

## Adding a vector

1. Add `<NNNN>-<name>.active` under the right tier.
2. Generate the text expectation through the shipped path (the test's
   `export` helper is the reference: `evaluate_with_seeds` +
   `SecretProfile::for_engine` + `value_to_json_with`). Never hand-write a
   secret-bearing expectation; the local test rejects a file that echoes a
   `secret("...")` literal.
3. If the published codec parses the document, add the codec expectation and
   confirm `cargo run --locked --manifest-path conformance/interop/checker/Cargo.toml`.
4. If it does not, leave it in `current/` and note the missing core feature in
   the pairing record's status section.

## Why the two sides differ

The layers are different by design:

- This workspace exports **evaluated** values: durations as
  `{"$active.duration":{"ms":N}}`, quantities in the base unit
  (`{"$active.quantity":{"unit":"bps","value":N}}`), and byte literals as
  integers (byte-ness is erased at evaluation, `spec/secrets.md` §2).
- The codec projects the **parsed module**: `{"value":N,"unit":"milliseconds"}`,
  `{"value":N,"unit":"megabits-per-second"}`, `{"value":N,"unit":"kibibytes"}`.

The checker normalizes both to `(base value, dimension)` before comparing, so
a rescale or reorder on either side fails instead of passing because the
spelling changed. Unknown units fail loudly; a new unit family must be added
to the checker's table deliberately.

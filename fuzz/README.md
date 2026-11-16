# Cargo-fuzz harness for ACTIVE (C-012, LANG-I-003)

The CI-enforced harness is the in-repo proptest suite
(`active-core/tests/fuzz.rs`). These cargo-fuzz targets drive the same
entry points with libFuzzer for deeper, longer-running fuzzing. CI runs
every target for **300 seconds** per push that touches the core or the
harness (eight targets in parallel, 45-minute job bound); the weekly
schedule repeats the same campaign.

## Targets

| Target | Entry points exercised |
| --- | --- |
| `lexer` | `Lexer::tokenize` on arbitrary bytes |
| `parser` | `parse` → `Engine::evaluate` |
| `interpolation` | expression/sub-parser paths |
| `formatter` | `parse` → `format` → re-parse, idempotence both styles |
| `convert` | all four importers (JSON/YAML/TOML/JSON Schema) → parse → evaluate |
| `secrets` | `secret()`, `secret.env()`, profiles, patches → redacted export; asserts no raw seed survives |
| `sign` | sidecar TOML + bytes → `verify_bytes` (identity, expiry, revocation parsing) |
| `formatter_eval` | format → re-parse → evaluate; asserts the value is unchanged |

## Seed corpora (`fuzz/corpus/<target>/`)

- `lexer/` 13 seeds, `parser/` 4 seeds (the historical crash reproducers
  included), `interpolation/` 4 seeds.
- `formatter/` 9 seeds: the canonical corpus
  (`tests/fmt/brace|indent/*.active`), the two examples, and all five
  committed formatter crash reproducers (`crash-*`).
- `formatter_eval/` 4 seeds, `convert/` 4 seeds (one per importer),
  `secrets/` 3 seeds (literal, env-bound, overlay), `sign/` 2 sidecar
  shapes (unbound and expiring).

Seed files are text so a reviewer can read what the fuzzer starts from.

## Historical discharge (audit #24)

The audit's formatter crash artifact was not retrievable (the CI artifact
expired), so the finding is discharged by a reconstruction: the four
real crash inputs were minimized and committed under
`fuzz/corpus/formatter/` (`crash-08de17d7…`, `crash-504a8155…`,
`crash-8e989b62…`, `crash-d1726a54…`, `crash-f438d190…`), each fixed with
a regression in the formatter, and the target replays them on every run.
A 300-second libFuzzer campaign on the formatter recorded 101,993 execs
with zero crashes (log in the fix record for Section D).

## Requirements

- `rustup toolchain install nightly`
- `clang` on PATH (libfuzzer-sys builds the libFuzzer C++ runtime)
- `cargo install cargo-fuzz --version 0.13.2 --locked`

## Usage

```sh
cargo +nightly fuzz run lexer -- -max_total_time=60
cargo +nightly fuzz run formatter -- -max_total_time=300
cargo +nightly fuzz run formatter_eval -- -max_total_time=300
```

Crashes and reproducers land in `fuzz/artifacts/` (gitignored); commit a
minimized reproducer under the target's corpus directory once fixed.

Measured smoke (2026-11-16, Docker `active-fuzz:1.88`, 20 s each, zero
crashes): `convert` 8,186 runs, `secrets` 1,318, `sign` 39,558,
`formatter_eval` 2,116 (peak RSS 137–434 MB). The CI campaign runs each
target for 300 s.

The Docker image `active-fuzz:1.88` carries the same toolchain used for
the recorded campaigns (`docker run --rm -v "$PWD:/w" -w /w
active-fuzz:1.88 bash -lc 'export PATH=/usr/local/cargo/bin:$PATH;
cargo fuzz run formatter -- -max_total_time=300'`).

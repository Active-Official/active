# Contributing to ACTIVE (C-064)

## Ground rules (non-negotiable)

These mirror the project's standing engineering rules:

1. **Spec before code.** Language changes land in `spec/` (or the
   relevant doc) first; spec and code never diverge.
2. **Full local gate** after every logical change (mirroring CI
   verbatim):
   `cargo build --workspace --locked`, `cargo clippy --workspace
   --all-targets --locked -- -D warnings`, `cargo fmt --all --check`,
   `cargo test --workspace --locked`, `ACTIVE check` on both examples.
   Zero warnings, fmt-clean, all green — or the change is not done.
3. **Independent review** before merge (Copilot CLI review or
   maintainer review — a norm enforced at merge, not a hook);
   findings triaged skeptically and fixed or rebutted in writing.
4. **Atomic conventional commits** (`feat/fix/spec/test/docs/chore/ci`
   expected by convention; maintainers enforce at merge); green CI
   on every push; never leave the branch red.
5. **Security invariants hold always:** secret redaction on every
   export path; hermetic, bounded evaluation; determinism.
6. **Non-goals stay out** (C-070): no Turing-complete computation,
   no side effects, no dynamic code loading, no unrestricted
   implicit coercion.

## Language changes need an RFC

Grammar, IR, evaluation semantics, diagnostics codes, CLI surface,
and conformance-affecting behavior require an accepted RFC first —
see `docs/rfcs/README.md`. Bug fixes that restore specified
behavior do not.

## Tests

Every change ships positive **and** negative tests on the real
shipped path (parse → evaluate → export), diagnostics with stable
codes and spans where applicable, and updated specs/docs. New
`.active` samples go through the corpus, docs-site, or schema-library
harnesses as appropriate — book samples that fail teach lies.

## Pull requests

Small, focused, one logical change. Describe what, why, how
tested, and any risks. Link the checklist item (`C-xxx`) or RFC.
The repository owner (see `.github/CODEOWNERS`) merges on green CI
+ approved review.

No Developer Certificate of Origin (DCO) sign-off is required:
contributions land under the repository license
(`MIT OR Apache-2.0`) via the GitHub Terms of Service.

## Maintainer ladder (L-024)

Contributor (docs, examples, schema entries, small fixes) to reviewer
(sustained quality + independent reviews) to owner (architecture calls,
releases). Each step is earned in public: merged work, review record,
then nomination. No step is for sale, and no step lowers the gate.

## Dogfood rule (L-022, standing)

New internal configs default to Active: budgets presets, conformance
manifests, repair benchmarks, tool parameters. When Active does not fit,
record the exception with reason. Current exceptions: repair and
conformance manifests stay TOML (consumed by Rust, Node, and Go runners
that must all parse them without an Active parser); token-corpus fixtures
stay multi-format by purpose; binding sources stay native to their
language. Exceptions are reviewed, never silent.

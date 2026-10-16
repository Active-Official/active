# RFC process (C-064)

Post-1.0 language changes go through Request For Comments. No RFC,
no language change — the process is the ratchet that keeps 1.0
stable while letting the language grow.

## When an RFC is required

Grammar, IR nodes, evaluation semantics, new diagnostic codes,
CLI surface changes, conformance-affecting behavior, MSRV bumps,
and version-policy changes. Not required: bug fixes restoring
specified behavior, performance work with no observable change,
docs, tests, tooling internals.

## Lifecycle

1. **Draft** — copy `docs/rfcs/0000-template.md` to
   `docs/rfcs/<nnnn>-<slug>.md` and open a pull request marked
   `rfc-draft`. Any community member may draft.
2. **Discussion** — at least 14 days of public review on the PR.
   Maintainers summarize objections; the draft evolves in place.
   **Fast-track lane:** small, compatible, or urgent fixes (typo-grade
   spec corrections, crash fixes with no surface change, security
   patches) may proceed on 72-hour lazy consensus with a single
   maintainer approval, documented in the PR.
3. **Decision** — the repository owner (see `.github/CODEOWNERS`)
   accepts, rejects, or requests changes, with written rationale
   addressing every raised objection.
   Accepted RFCs get a number, a target release, and checklist
   items; the RFC PR closes at acceptance and the implementation
   PR links it (implementation stays open until merge).

## Rejection without prejudice

Non-goals (C-070) close fast with a pointer: no Turing-complete
computation, no side effects, no dynamic code loading, no
unrestricted implicit coercion, never a data-transformation
language.

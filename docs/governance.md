# Active governance (CHECKLIST F.008, PROPOSAL §6.5)

Who decides what, and how the decision is recorded. Short on purpose:
process serves the language, never the reverse.

## 1. RFC process

- Any schema, ABI, diagnostic-code, or CLI-surface change starts as
  an RFC in `docs/rfcs/` (template `0000-template.md`; README there
  defines the states). No RFC, no change — the checklist enforces
  it by requiring a proposal back-reference for every step.
- Small, additive, obviously-correct changes (typo fixes, new tests,
  version bumps) ride the normal review loop without an RFC; the
  reviewer may still demand one.
- Acceptance needs: a senior-architect review pass (copilot-review
  with reasoning effort `xhigh` on normative surfaces), green full
  gates on a clean tree, and dual-push with green CI on both
  remotes. Security/FFI/budget/secret-boundary changes additionally
  need a BYOK meta-muse review recorded with its outcomes.
- Rejected directions stay recorded: the RFC keeps the rationale
  so the same idea is not re-litigated without new evidence.

## 2. Deprecation policy

Codes, schema constructs, bindings, and CLI flags deprecate per
`docs/deprecation.md` (codes are API: never reused, additive in
1.x, announced replacement plus one minor of overlap, removal at a
major at the earliest). Deprecating a binding additionally requires:
the matrix row flips to its terminal status with date and pointer,
the conformance job drops its path the same commit, and the
install surface stops routing to it.

## 3. Ownership

| Artifact | Owner | Notes |
|---|---|---|
| Support matrix | `bindings/README.md` (language team) | Flips only with date + evidence pointer; Appendix B defines Complete |
| Capability descriptor | `agent-kit/capability.schema.json` | Versioned; consumers reject unknown majors (`active capability --require-major`) |
| Conformance contract | `conformance/NORMALIZATION.md` + harness | Versioned; gate fails closed on drift |
| Error catalog | `docs/diagnostics.md` + `conformance/errors/` | Codes never reused; catalog diff green |
| Schema library | `schemas/` | Additive within a minor; fixtures + docgen pins |
| Prompt pack | `prompts/` (VERSION + per-file header) | Lockstep with codes/constructs; measured rounds committed |
| Release line | workspace `Cargo.toml` + `packaging/` | Single-sourced version; SBOM snapshot per release |

Ownership means keeping the artifact true, not merely holding the
pen: the owner updates it in the same commit as the change it
describes, and review rejects drift between code and record.

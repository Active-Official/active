# Agent rules for the Active repository

Active is the Config + Data language for humans and agents. The Rust
workspace in this directory is the reference implementation. These rules
apply to every AI agent working here.

## Read first

- `spec/` is normative: grammar, validation, secrets, budgets, blobs,
  converters, tooling surfaces, and the agent interchange profile.
- `docs/tooling.md` and `docs/Agent-Handbook.md` cover the shipped tool
  surfaces and the local working agreement.
- `master-program/` holds the execution program (proposal, checklists,
  gap register). It is working material; never commit it or push it to a
  remote. `master-program/CHECKLIST.md` and
  `master-program/TOOLING-CHECKLIST.md` are the traceability record.

## Build and gates (all must pass before any commit)

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Run the targeted suite for the crate you touched first, then the full
workspace. Additional gates:

- `python agent-kit/eval/test_capability.py` — descriptor authority.
- `python conformance/harness/run.py` — cross-embedding conformance
  (Docker image `active-all-toolchains:phase2` for the full matrix).
- `cargo test -p active-ffi` — includes the committed-header drift gate.
- `cargo check --manifest-path fuzz/Cargo.toml` — fuzz targets compile.

## Invariants (never weaken to make a test pass)

- Secrets are structural: `secret(...)`, env-name aliases, locator
  subtrees, ancestors, and profile-override selections are material in
  every output path (eval, convert, doc, lock, audit, FFI, WASM, LSP,
  viz, traces, content hashes).
- Budgets are hard ceilings and fail closed. A truncated walk reports
  unknown paths as material, never as benign.
- Determinism: same source + budget + profile + mode produces the same
  bytes and the same `content_hash`.
- Mode is explicit: Config is the default; Data is opt-in; both keep
  identical safety invariants.
- Diagnostic codes are stable API. Add codes in documented namespaces;
  never reuse or renumber.
- Conformance vectors travel with behavior: a new code, flag, or
  capability lands with vectors and docs in the same change.
- Headless surfaces emit stable JSON; human surfaces stay readable.

## Code conventions

- Keep pure logic free of I/O; test through the real entry point
  (CLI command, FFI call, LSP handler, harness), not a re-implementation.
- One canonical implementation per capability. Upgrade the existing
  tool; do not fork a parallel one.
- Comments explain constraints that are not obvious from the code.
- Commit messages use conventional prefixes (`feat:`, `fix:`, `spec:`,
  `test:`, `docs:`, `chore:`) and say what changed and why.

## Secrets and external systems

- Registry tokens and API keys live outside the repository and are read
  from the environment. Never print, log, commit, or paste them.
- Publishing to npm, PyPI, or crates.io requires explicit owner
  approval. The project identity is `active-lang`.
- Dual remotes: `origin` (RatioArtificiosa/active-lang) and `official`
  (Active-Official/active). Both must be green before a release claim.

# Versioning policy — Active (active-lang)

> **Status:** decided 2026-11-14 by the active-lang ultra audit and merged here
> with the fixes it governs. `AGENTS.md` and `CONTRIBUTING.md` reference this
> file; where any other document disagrees with it, this file governs and the
> other is fixed in the same change.

## 1. The freeze

No version bumps and no publishes to crates.io, PyPI, or npm until all of the
following hold:

1. This file is merged and referenced from `AGENTS.md` and `CONTRIBUTING.md`.
2. The published maxima in §3 are verified against the registries; no crate is
   bumped from a repository `Cargo.toml` version — only from its published
   maximum.
3. Lockstep is wired: `active-core/tests/packaging.rs` (workspace version,
   Tier-1 versions, the VS Code extension) is green, covers `active-ffi` and
   `active-wasm`, and the placeholder pin is replaced per §4.
4. CI gates are live and green: `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace` (on the MSRV and on stable),
   `python agent-kit/eval/test_capability.py`,
   `python conformance/harness/run.py`, the packaging gate (full
   `cargo package`), and the MSRV gate (build **and** test on the floor).
5. Public presence is clean: README claims true, CHANGELOG honest, policy
   docs reconciled, docs site versioned, npm placeholder deprecated.
6. The first post-freeze release is prepared: CHANGELOG entry with the
   breaking-change list (§6), a single `v2.0.0` tag, and a
   `cargo publish --dry-run` for every crate in §5.
7. The owner approves that first release explicitly.

**Emergency exception:** a security fix or yank-worthy defect may publish as a
patch during the freeze, only with explicit owner approval for that release,
only as a patch version, and only with the same procedure (§5).

The freeze never blocks code fixes, tests, commits, GitHub pushes, or CI. It
blocks only version bumps and registry publishes.

## 2. One version, one line

- One workspace version (`[workspace.package] version`); every member crate
  uses `version.workspace = true`. All crates release together.
- Tags are `v<version>` on `main` and drive `release.yml`.
- The current line is **1.4.0** (frozen). The next line is **2.0.0** (§6).
  There will never be a 1.5.0.
- SemVer strictly: no leading zeros, prerelease tags only for actually
  published pre-releases, never for staging placeholders (§8).
- MSRV advances only with a minor/major bump and a checklist note (§7).

## 3. Lockstep — which crates move together, and the published maxima

| Crate / package | Registry | First published | Published max | Notes |
|---|---|---|---|---|
| `active` | crates.io | 0.1.0 (2026-10-16) | **1.4.0** (2026-10-20) | Facade over `active_core`. No 1.0.0 ever published. |
| `active-core` | crates.io | 0.1.0 (2026-10-16) | **1.4.0** (2026-10-20) | No 1.0.0 ever published. Download counts are not adoption evidence. |
| `active-cli` | crates.io | 0.1.0 (2026-10-16) | **1.4.0** (2026-10-20) | Binary `active`. |
| `active-fmt` | crates.io | 0.1.0 (2026-10-16) | **1.4.0** (2026-10-20) | |
| `active-lsp` | crates.io | 0.1.0 (2026-10-16) | **1.4.0** (2026-10-20) | |
| `active-ffi` | crates.io | **1.4.0** (2026-10-20) | **1.4.0** | First published at 1.4.0. |
| `active-wasm` | crates.io | **1.4.0** (2026-10-20) | **1.4.0** | First published at 1.4.0. |
| `active-lang` | PyPI | **1.4.0** (2026-10-22) | **1.4.0** | Only 1.4.0 exists. |
| `active-lang` | npm | `0.0.0-stage` then 1.4.0 (2026-10-22) | **1.4.0** | `0.0.0-stage` is a known stray (staging placeholder), **deprecated 2026-11-17**: `npm deprecate active-lang@0.0.0-stage "Staging placeholder; use active-lang@1.4.0 (or 2.0.0 after the freeze lifts)"`. Verified on the registry (`versions["0.0.0-stage"].deprecated`); `latest` remains 1.4.0. Never stage placeholders publicly again (§8). |
| VS Code extension (`editors/vscode`) | GitHub release asset (`.vsix`) | 1.1.0 | **1.4.0** | Version tracks the workspace (pinned by the packaging test). |

Known gaps in the record (do not "fix" by rewriting history):

- npm `active-lang@0.0.0-stage` is a stray staging placeholder published
  ~2 minutes before 1.4.0; it is deprecated as of 2026-11-17 (message in
  the npm row above).
  Download counts are not adoption evidence anywhere in this table
  (LANG-J-015).

- 0.1.0 has no source commit or tag; it was published from an uncommitted
  tree. It stays published; the provenance gate (§5.3) exists so this cannot
  happen again.
- 1.0.0 was declared stable by tag but never published anywhere. Do not
  retro-publish it.
- `active-infinity` is NOT in this table. See §4.

Also pinned to the workspace version on every release (enforced by
`active-core/tests/packaging.rs`): `bindings/python/pyproject.toml`,
`bindings/js/package.json`, the runtime version fallbacks in
`bindings/python/active.py` and `bindings/js/active.js`, and
`agent-kit/capability.schema.json`.

## 4. The `active-infinity` placeholder — RETIRED (DECIDED)

- **What happened:** the in-repo member `active-infinity` (version `1.0.0`,
  one `VERSION` const, "reserves the name … converges to the workspace
  version when the real crate lands") was published to crates.io on
  2026-10-19 as 1.0.0. The same name is now the real Infinity codec,
  published **2.4.2 on 2026-11-03** (canonical home
  `Active-Official/active-infinity`), with 13,822 downloads against the
  placeholder's 12. One crate name, two different crates (audit finding
  LANG-E-001, Critical). The placeholder's convergence premise is false: the
  real crate already landed, under the same name, on a different version
  track.
- **Decision (2026-11-14): retire the member.** The `active-infinity/`
  directory is deleted, the member is removed from the workspace `members`
  list, and the packaging test asserts the member stays gone (a directory
  named `active-infinity/` at the workspace root fails the test). The
  canonical-home sentence lives in `docs/version-and-abi-policy.md §1` and
  `docs/release-checklist.md` step 5.
- **Yank nothing.** The published placeholder 1.0.0 stays published: yanking
  would break its 12 downloaders and would not remove the collision — the
  record stays either way.
- **Never re-add** an `active-infinity` member to this workspace. The name
  `active-infinity` on crates.io is the Infinity codec; references to it
  from this repository point at `Active-Official/active-infinity`.
- **Defensive reservations:** `active-ls` and `active-fuzz` are held on
  crates.io (both return HTTP 200; ownership to be confirmed by the owner).
  This file is the reservation record.

## 5. Release procedure (every release, no exceptions)

1. **CHANGELOG first**, with what changed and why it matters for users.
2. **Single tag** `vX.Y.Z` on `main`, on the exact commit being released.
3. **Provenance gate.** The release workflow must fail unless the working
   tree is clean, the checked-out commit equals the tag's commit, and the
   workspace version from `cargo metadata` equals the tag without its `v`.
4. **Dry-run.** `cargo publish --dry-run` for every crate in §5 order, plus
   `cargo package --locked` for each (full packaging, not `--list`).
5. **Publish in dependency order with index-lag pauses:**
   `active-core → active → active-fmt → active-lsp → active-cli →
   active-ffi → active-wasm`, then PyPI and npm. A publish step fails loudly
   when publishing is unavailable; it never silently skips a registry and
   exits 0.
6. **GitHub release, same tag,** with all binary archives, `SHA256SUMS`,
   provenance attestations, the VS Code `.vsix`, the WASM bundle, and the
   conformance archive. A missing artifact blocks the release.
7. **Owner approval** for the first post-freeze release; publishing to any
   registry always requires explicit owner approval.
8. **Verify after.** Both remotes agree, the registries show the new
   version, the release exists with all assets, the docs site states it, and
   CI is green on both orgs before any release claim.

**Matching rule:** crates.io versions and GitHub tags/releases must match —
one tag and one release per published version. A published version with no
tag, a tag with no published version, and a tag with no release are defects,
not bookkeeping gaps.

## 6. First post-freeze release: 2.0.0 (DECIDED)

The first post-freeze release is **2.0.0**, not 1.5.0: the fixes the audit
proves necessary change the JSON wire format, `content_hash` identity, the
lockfile format, signature verification, documented evaluation behaviour, and
the MSRV — a set the 1.x "avoids breaking changes by policy" promise cannot
absorb.

Breaking changes the 2.0.0 release notes must enumerate:

- JSON wire format: durations/quantities/blobs gain typed wrappers.
- `content_hash` values change for documents containing non-finite floats.
- Lockfile `value_hash` format becomes salted per lockfile.
- Signatures bind path/role; old versions stop verifying.
- `requires` is enforced (previously unconditional).
- Non-finite floats are diagnosed instead of silently becoming null.
- Parse and evaluation amplification bounds; previously accepted documents
  may now be refused.
- Cross-table references resolve.
- L-P09 unification: includes are equal-priority peers (last-include wins
  is gone; two includes that disagree are E046), repeated table bodies
  merge instead of last-wins/W011 (same-priority disagreement is E046),
  profiles merge at Profile priority with W003 retired, and
  `active inspect --path` reports value report v2 with merge provenance.
- `active compatibility` detects numeric-bound/pattern/format/closed
  tightenings.
- MSRV 1.85 → 1.89.
- `active-infinity` placeholder member retired (LANG-E-001).

Unchanged across the move: the C ABI stays at **v1**; diagnostic codes are
never reused or renumbered (new codes are additive); the 1.x line ends at
1.4.0.

Policy statements updated for 2.x: the 2.x line avoids breaking changes by
policy within a major — diagnostic codes are never reused, changes are
additive within 2.x, the C ABI stays at v1, and the changelog is honest
about every release.

## 7. MSRV

- The current published MSRV is **1.85**; it was a build floor only, and the
  test suite's dev-dependencies already required newer rustc.
- **2.0.0 raises the MSRV to 1.89.0**: root `rust-version = "1.89"`, the CI
  MSRV gate runs build **and** `cargo test` on 1.89.0, and README/docs-site
  MSRV lines state 1.89.
- After 2.0.0 the MSRV gate keeps testing the floor on the pinned 1.89.0
  toolchain; the main test job tracks stable so a newer-stable regression
  is caught early.

## 8. Public presence rules

- **README:** stability, MSRV, changelog-honesty, and benchmark claims must
  be true at release time; an aspiration says so.
- **crates.io READMEs:** short, no scaffolding, no TODOs, no "continuing
  work" notes.
- **Docs site:** every release updates the documented version; no page
  describes a shipped release as "upcoming".
- **Staging placeholders:** never publish a placeholder to a public
  registry; stage locally or under an uninstallable scope.
- **Download counts are not adoption evidence.**
- **Version/ABI contract:** `docs/version-and-abi-policy.md` is the
  compatibility contract; it must agree with this file.

## 9. What this file does not do

- It does not authorise any publish: publishing always requires explicit
  owner approval, and the first post-freeze release also requires the
  owner's approval for that release.
- It does not change the push discipline (double-push both orgs, CI green on
  both before a release claim).
- It does not decide the transport architecture; that decision lives in its
  own record, and this file records only its versioning consequences.

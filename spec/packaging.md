# Packaging, Distribution & Versioning (C-056/C-057, P-054)

How ACTIVE ships: crate names, version policy, release binaries,
signing, crates.io publication, and editor distribution.

## 1. Version policy

- One workspace version (`[workspace.package] version`): currently
  `1.4.0`. All crates release together; no independent crate versioning.
  The version/ABI contract for the language packages lives in
  `docs/version-and-abi-policy.md`; this section records the Rust
  workspace mechanics.
- SemVer: `0.x` alphas may break the surface (documented in
  CHANGELOG entries); `1.0.0` ships with C-090 and freezes the
  surface (grammar + IR + diagnostics codes + CLI flags).
- MSRV (`rust-version = 1.85`) advances only with a minor/major
  bump and a checklist note, never silently.
- Tags: `v<version>` on main cut releases (drives release.yml).

## 2. Crate names (package vs binary vs library)

crates.io namespaces are first-come: the `active` family
(`active`, `active-core`, `active-cli`, `active-fmt`, `active-lsp`)
is owned by RatioArtificiosa, with `active-ls` and `active-fuzz`
reserved defensively (see RENAME-STATUS in the Active workspace).
The Rust library names track the packages — only manifests move.

| Package (crates.io) | Library / binary | Installs |
|---|---|---|
| `active` | lib `active` (facade over `active_core`) | `use active::…` |
| `active-core` | lib `active_core` | `use active_core::…` |
| `active-cli` | bin `active` | `cargo install active-cli` |
| `active-fmt` | bin `active-fmt` (+ lib `active_fmt`) | `cargo install active-fmt` |
| `active-lsp` | bin `active-lsp` (+ lib `active_ls`) | editor servers |

Dependents write `active-core = "1"` and `use active_core::…`
(or `active = "1"` with `use active::…` for the facade).
Binary names (`active`, `active-fmt`, `active-lsp`) are install-local
and never collide on crates.io.

## 3. Release binaries (release.yml, on `v*` tags)

Six artifacts: Linux/macOS/Windows × x86_64/ARM64, each bundling
`samples` evaluation parity (`ACTIVE check` on both examples runs
per target before archiving). Every artifact ships:

- `ACTIVE-<target>.tar.gz` / `.zip` (Windows) with `ACTIVE`,
  `active-fmt`, `active-lsp`, `README.md`, `LICENSE-*`, and `schemas/`
  (the standard library, vendored for offline use).
- `SHA256SUMS` over all archives.
- GitHub build-provenance attestations (keyless OIDC,
  `actions/attest-build-provenance`) — the primary authenticity
  story, verifiable with `gh attestation verify`.
- Optional `ACTIVE-sign` Ed25519 sidecars (`.sig`): the workflow
  signs with `ACTIVE sign` (C-052 dogfood) when the
  `RELEASE_SIGNING_KEY` secret is present — its content is the
  `.secret` file text from `ACTIVE keygen`, sidecars record kid
  `ACTIVE-release`. Verifiers check against a trust file holding
  that id and the release public key (published in `docs/`
  alongside). Without the secret the step is skipped and
  attestations + checksums carry verification.

Matrix (native runners where offered, cross otherwise — CI is
authoritative; documented here when a leg needs a fallback):

- `x86_64-unknown-linux-gnu` (ubuntu-24.04)
- `aarch64-unknown-linux-gnu` (ubuntu-24.04-arm, else cross)
- `x86_64-pc-windows-msvc` (windows-11)
- `aarch64-pc-windows-msvc` (windows-11-arm, else documented gap)
- `aarch64-apple-darwin` (macos-15)
- `x86_64-apple-darwin` (cross from macos-15)

## 4. crates.io publication

Order matters (path deps rewrite to version deps): `active-core`
first, then `active-fmt`, `active-lsp`, `active-cli`. Continuous gate
(`package` job in ci.yml): full `cargo package` for
`active-core` (proves publish-readiness end to end) plus
`cargo package --list` for the dependents (file inclusion +
manifest parse). Full packaging of the dependents becomes possible
— and the CI job extends to it — the moment `active-core` is
published, since cargo verifies path deps against the registry.

Two supported paths for the real publish (both need a one-time
human step — publication is authenticated, automation cannot mint
it):

1. **Trusted Publishing (preferred, no tokens):** link the repo on
   crates.io (`active-core`, `active-cli`, `active-fmt`,
   `active-lsp` → GitHub OIDC for `Active-Official/active`);
   release.yml then publishes passwordlessly on tags.
2. **Token:** `cargo login` with `CARGO_REGISTRY_TOKEN`; release.yml
   publishes when the secret exists, else skips with a notice.

Verify locally any time (no network writes):

```bash
cargo package --list -p active-core   # file list
cargo publish --dry-run -p active-core
```

## 5. Editor distribution (C-057)

- **VS Code / Cursor / VSCodium** (one vsix serves all three —
  Cursor and VSCodium consume the VS Code marketplace format):
  `editors/vscode/` (`package.json` + TextMate grammar +
  dependency-free JS client spawning `active-lsp`). `vsce package`
  builds `active-lang-<version>.vsix` (attached to GitHub releases
  by release.yml; never committed). Publishing (`vsce publish`)
  needs a marketplace token — human step, procedure in
  `editors/vscode/README.md`.
- **Neovim**: `editors/neovim/` is a runtimepath plugin
  (`ftdetect/`, `ftplugin/`, `syntax/ACTIVE.vim`, `doc/ACTIVE.txt`)
  installable via any plugin manager (`lazy.nvim`, `packer`,
  `vim-plug` one-liners in its README) plus an `lspconfig`
  snippet pointing at `active-lsp`. No registry, no token.

## 6. Install surface (user-facing summary)

- `cargo install active-cli active-fmt` + `active-lsp` via the vsix or
  `cargo install active-lsp`; library via `active-core = "1"`.
- Release archives for users without Rust; checksums +
  attestations (+ `.sig` sidecars when the release key is set).

## 7. Release cadence (L-024)

Patch releases ship on demand for correctness fixes (no behavior change
beyond the fix, full gates, changelog entry). Minor
releases bundle additive surface (new crates, bindings, schema domains)
with conformance-plus-release notes. Majors only for breaking changes,
which the 1.x line avoids by policy (codes additive, ABI versioned).
Every release runs the release checklist (G-011a): conformance artifact,
native matrix, wasm blob, checksums, changelog honesty. The cadence is
demand-driven, never calendar-driven; a quiet month with green CI is a
healthy month.

## 8. Supersession note (2026-10-21, Phase-2 expansion)

Sections above describe the 1.x-era Rust-crate surface and stay
unchanged as history. The Tier-1 language packages (PyPI
`active-lang`, npm `active-lang` (`@active-lang/active` reserved as fallback), Go module), the
version/ABI compatibility contract, the install surface, and the
supply-chain baseline now live in
`docs/version-and-abi-policy.md`, `docs/install.md`, and
`packaging/` — those files govern, this one records. Historical
namespace references (`solid-lang` URLs) are redirect-safe and
intentionally untouched.

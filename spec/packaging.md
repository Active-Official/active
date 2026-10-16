# Packaging, Distribution & Versioning (C-056/C-057, P-054)

How SOLID ships: crate names, version policy, release binaries,
signing, crates.io publication, and editor distribution.

## 1. Version policy

- One workspace version (`[workspace.package] version`): currently
  `0.1.0-alpha.1`. All crates release together; no independent
  crate versioning before 1.0.
- SemVer: `0.x` alphas may break the surface (documented in
  CHANGELOG entries); `1.0.0` ships with C-090 and freezes the
  surface (grammar + IR + diagnostics codes + CLI flags).
- MSRV (`rust-version = 1.85`) advances only with a minor/major
  bump and a checklist note, never silently.
- Tags: `v<version>` on main cut releases (drives release.yml).

## 2. Crate names (package vs binary vs library)

crates.io namespaces are first-come: `solid-core` belongs to an
unrelated 2020 crate, so the core ships under a scoped name. The
Rust library name is unchanged — only manifests move.

| Package (crates.io) | Library / binary | Installs |
|---|---|---|
| `solid-lang-core` | lib `solid_core` | `use solid_core::…` |
| `solid-cli` | bin `solid` | `cargo install solid-cli` |
| `solid-fmt` | bin `solid-fmt` | `cargo install solid-fmt` |
| `solid-ls` | bin `solid-ls` (+ lib `solid_ls`) | editor servers |

Dependents write `solid-lang-core = "0.1"` (tracking the current
0.1 line; `= "1"` at 1.0) and `use solid_core::…`.
Binary names (`solid`, `solid-fmt`, `solid-ls`) are install-local
and never collide on crates.io.

## 3. Release binaries (release.yml, on `v*` tags)

Six artifacts: Linux/macOS/Windows × x86_64/ARM64, each bundling
`samples` evaluation parity (`solid check` on both examples runs
per target before archiving). Every artifact ships:

- `solid-<target>.tar.gz` / `.zip` (Windows) with `solid`,
  `solid-fmt`, `solid-ls`, `README.md`, `LICENSE-*`.
- `SHA256SUMS` over all archives.
- GitHub build-provenance attestations (keyless OIDC,
  `actions/attest-build-provenance`) — the primary authenticity
  story, verifiable with `gh attestation verify`.
- Optional `solid-sign` Ed25519 sidecars (`.sig`): the workflow
  signs with `solid sign` (C-052 dogfood) when the
  `RELEASE_SIGNING_KEY` secret is present — its content is the
  `.secret` file text from `solid keygen`, sidecars record kid
  `solid-release`. Verifiers check against a trust file holding
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

Order matters (path deps rewrite to version deps): `solid-lang-core`
first, then `solid-fmt`, `solid-ls`, `solid-cli`. Continuous gate
(`package` job in ci.yml): full `cargo package` for
`solid-lang-core` (proves publish-readiness end to end) plus
`cargo package --list` for the dependents (file inclusion +
manifest parse). Full packaging of the dependents becomes possible
— and the CI job extends to it — the moment `solid-lang-core` is
published, since cargo verifies path deps against the registry.

Two supported paths for the real publish (both need a one-time
human step — publication is authenticated, automation cannot mint
it):

1. **Trusted Publishing (preferred, no tokens):** link the repo on
   crates.io (`solid-lang-core`, `solid-cli`, `solid-fmt`,
   `solid-ls` → GitHub OIDC for `RatioArtificiosa/solid-lang`);
   release.yml then publishes passwordlessly on tags.
2. **Token:** `cargo login` with `CARGO_REGISTRY_TOKEN`; release.yml
   publishes when the secret exists, else skips with a notice.

Verify locally any time (no network writes):

```bash
cargo package --list -p solid-lang-core   # file list
cargo publish --dry-run -p solid-lang-core
```

## 5. Editor distribution (C-057)

- **VS Code / Cursor / VSCodium** (one vsix serves all three —
  Cursor and VSCodium consume the VS Code marketplace format):
  `editors/vscode/` (`package.json` + TextMate grammar +
  dependency-free JS client spawning `solid-ls`). `vsce package`
  builds `solid-lang-<version>.vsix` (attached to GitHub releases
  by release.yml; never committed). Publishing (`vsce publish`)
  needs a marketplace token — human step, procedure in
  `editors/vscode/README.md`.
- **Neovim**: `editors/neovim/` is a runtimepath plugin
  (`ftdetect/`, `ftplugin/`, `syntax/solid.vim`, `doc/solid.txt`)
  installable via any plugin manager (`lazy.nvim`, `packer`,
  `vim-plug` one-liners in its README) plus an `lspconfig`
  snippet pointing at `solid-ls`. No registry, no token.

## 6. Install surface (user-facing summary)

- `cargo install solid-cli solid-fmt` + `solid-ls` via the vsix or
  `cargo install solid-ls`; library via `solid-lang-core = "1"`.
- Release archives for users without Rust; checksums +
  attestations (+ `.sig` sidecars when the release key is set).

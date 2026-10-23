# Distribution matrix (CHECKLIST 3.004, PROPOSAL section 5.4)

State verified 2026-10-22 against the public registries.

| Target | Package | Status | Artifact / source |
|---|---|---|---|
| crates.io | `active-core` + 6 workspace crates | **live at 1.4.0** | `cargo publish` in dependency order (release.yml) |
| npm | `active-lang` | **live at 1.4.0** | `bindings/js` wrapper + shim; `@active-lang/active` reserved as scoped fallback |
| PyPI | `active-lang` | **live at 1.4.0** | `bindings/python` + wasmtime; OIDC trusted publishing (`pypi.yml`) |
| GitHub Releases | per-platform archives, WASM bundle, SBOMs | **live** | `release.yml` on `v*` tags: checksums, attestations, optional `.sig` |
| VS Code Marketplace / vsix | `active-lang` extension | packaged per release | `editors/vscode` + `vsce package` job |
| Go modules | `github.com/Active-Official/active-go` | external module, pinned commit `f5bd6e2` | wazero-based, zero cgo; harness path `go-native` |
| Neovim | runtimepath plugin | shipped in-tree | `editors/neovim/` |
| Maven / GitHub Packages | unclaimed | Tier-2 smoke proof only | Java binding SPEC; no artifact |
| NuGet | unclaimed | Tier-2 smoke proof only | C# binding; no package yet |
| RubyGems / Packagist / others | unclaimed | Tier-2 smoke proof only | no artifacts |

The universal per-release contents are: platform archives,
`active_wasm.wasm`, the hand-written JS/TS shim, the conformance tarball,
and the capability manifest (`agent-kit/capability.schema.json`).

Rule: no registry is touched without an explicit release act naming the
account and the version. Names stay reserved in prose until the publish
they describe actually happens; a merged commit never publishes.

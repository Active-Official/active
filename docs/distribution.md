# Distribution matrix (CHECKLIST 3.004, PROPOSAL section 5.4)

Shipped today: crates.io (7 crates at 1.4.0), GitHub releases with
signed archives, checksums, build attestations, WASM bundle, and the
VS Code extension. The universal per-release contents are: platform
archives, `active_wasm.wasm`, the hand-written JS/TS shim, the
conformance tarball, and the capability manifest
(`agent-kit/capability.schema.json`).

Staged, awaiting owner credentials and final names (publishing to a
public registry is a point of no return and is not done unilaterally):

| Target | Package name status (checked 2026-10-21) | Ready artifact |
|---|---|---|
| PyPI | `active-lang` free | `bindings/python` + wasmtime dep; needs `pyproject.toml` + publish token |
| npm | `active-lang` free (plus `@active-lang/active` fallback) | `bindings/js` wrapper + shim; published with owner token |
| Go modules | proxy serves any repo path | external `active-go` repo; needs a tagged release there |
| Maven / GitHub Packages | unclaimed | Java binding is SPEC-only; no artifact yet |
| NuGet | unclaimed | C# smoke proven; needs `.nuspec`/SDK pack + API key |
| RubyGems / Packagist / others | unclaimed | bindings are SPEC-only; no artifacts yet |

Rule: no registry is touched without the owner's explicit go-ahead
naming the account and the version. Reservation squatting without a
shipped artifact violates registry policy; names stay reserved in
prose until the publish it describes actually happens.

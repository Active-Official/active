# Packaging helpers (Phase-2 D.002–D.007)

Repo-relative scripts plus retained snapshots. Nothing here runs at
import time; CI and the release job call these explicitly.

- `fetch-wasm.py` — download the release `active_wasm.wasm` for a
  workspace version, verify it against the release `SHA256SUMS`,
  and place it where Tier-1 bindings resolve it as the bundled
  module (`--into bindings/python bindings/js`). Maintainer-run
  when building distributables; never fetched silently at install.
- `assemble-universal.py` — build the manual-wiring tarball
  (`active-universal-<version>.tar.gz`: `active.h`, native libs,
  WASM, wiring README) from local release builds. Maintainer-run
  per release (see below); the produced tarball is evidence, not
  committed (multi-MB binary).
- `sboms/` — committed CycloneDX snapshot for the workspace at the
  current release line, generated with `cargo cyclonedx --all
  -f json` (cargo-cyclonedx 0.5.9, spec 1.3). Regenerate on every release
  and after dependency changes:
  `cargo cyclonedx --all -f json`, then move the per-crate
  `<crate>.cdx.json` files here after replacing the local checkout
  prefix in `bom-ref` URIs with `file://ACTIVE_REPO` (keeps
  machine paths out of git; timestamps stay as generated). The
  release job generates fresh copies into `dist/` (same tool, pinned
  version), where they join `SHA256SUMS`, build-provenance
  attestations, and the release upload. SBOM freshness on main is
  covered by that release-time regeneration, not by a per-push
  check.

Version rule: everything stamped here tracks `[workspace.package]
version` (pinned by `active-core/tests/packaging.rs`
`tier1_package_versions_track_workspace`).

# Release checklist (G-011a)

Every `v*` tag run must produce, in order:

1. Native matrix binaries (release.yml `build`), each with samples parity
   (`active check` on both examples per target before archiving).
2. WASM bundle (`wasm` job): module, browser + node glue, shim with
   declarations; node conformance runner green on the built blob.
3. Conformance archive (`active-conformance.tar.gz`) from the frozen
   manifest; reference runner green in the same run.
4. Checksums (SHA256SUMS) plus build attestations plus `.sig` sidecars when
   the release key is set.
5. GitHub release with all of the above attached; crates published in
   dependency order with index-lag pauses (active-core first: active-core ->
   active -> active-fmt -> active-lsp -> active-cli -> active-ffi ->
   active-wasm). There is no active-infinity publish from this repo - that
   name belongs to the standalone Infinity codec. The verified triple is
   recorded in docs/interop-pairing.md and re-verified on every active-core
   minor bump.
6. Changelog entry that says what changed and why it matters for users,
   not only what changed.

A missing artifact blocks the release; a green run without all six is not
a release.

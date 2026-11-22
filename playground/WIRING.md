# Playground wiring contract (L-002 MVP, implemented by W-002)

`playground.js` talks ONLY to `window.ActiveAPI`. The active-wasm JS/TS shim
must provide exactly this shape; no evaluation logic may live in page JS.

```ts
window.ActiveAPI = {
  // Evaluate Active source. Never throws for language errors: they come
  // back as { ok: false, json: "", diagnostics: "<code+span lines>" }.
  // Host-side failures (OOM, engine missing) may throw Error.
  evaluate(source: string): {
    ok: boolean;
    json: string;
    diagnostics: string;
    // Structured diagnostics, passed through verbatim when the module
    // carries them and ABSENT otherwise: the bindings fail closed on a
    // missing array, so the shim never invents an empty one.
    items?: Array<{
      code: string;
      message: string;
      severity: "error" | "warning" | "info" | "hint";
      span: {
        line: number;
        column: number;
        end_line: number;
        end_column: number;
      } | null;
    }>;
  };
  // L-P47: BLAKE3 hex digest of the exact UTF-8 source bytes. A module
  // without `active_blake3` throws (fail loud, never guess a hash).
  hash(source: string): string;
}
```

- `json` is the `active eval` JSON with secrets redacted (same bytes the CLI prints).
- `diagnostics` is one `file:line:col: [CODE] message` line per diagnostic, same order as CLI stderr.
- Secrets: the shim must never log source or values; redaction happens inside the WASM module.

## Local-dev loop (the release also ships a runnable bundle; G-023)

1. Build the workspace WASM module (W-001) for `wasm32-unknown-unknown`,
   then stage it next to the page:
   `cargo build -p active-wasm --target wasm32-unknown-unknown --release`
   then `python playground/fetch-wasm.py`. The script writes the gitignored
   `playground/active_wasm.wasm` and verifies the copy
   (`--check` re-verifies it). To stage a published build instead, run
   `python packaging/fetch-wasm.py --version <v> --into playground`, which
   verifies the release checksum.
2. Serve this directory statically (`python -m http.server`); `fetch()` needs http, not `file://`.
3. Load `index.html`; without the engine the page says "Engine not loaded" (honest empty state, never faked output).

## Release bundle (G-023)

The release wasm job stages `playground/active_wasm.wasm` from the same
`dist-wasm/` bundle and publishes `active-playground.tar.gz`. Extract the
archive, serve its root statically (`python -m http.server`), and open
`/playground/index.html`: the page finds its engine, the shim, and
`playground.js` with no local build. The archive is checksummed in
`SHA256SUMS` like every other release file; its engine bytes are the
same `active_wasm.wasm` the release publishes.

## Hash-addressed snippets (L-P47)

The page shares a snippet as `#blake3:<64 hex>:<base64url source>` and
verifies the hash before showing or evaluating anything; a mismatch is a
refusal (the editor is cleared, nothing runs, nothing is cached). A
hash-only `#blake3:<hash>` resolves from the bounded local cache
(`active.snippet.<hash>`, 64 entries / 1 MiB, re-verified on read); a
miss refuses visibly and never fetches from the network. Self-contained
links are the portable share; the cache is per browser. `snippets.js`
holds the fragment/cache logic and `test-snippets.js` gates it against
the release wasm. The full rules are in `spec/playground.md`.

## Share links (G-019 decision)

Links carry `#c=` deflate-raw-compressed source (platform
CompressionStream, base64url) or `#s=` raw encoded source, whichever is
shorter (compression overhead loses on tiny documents). Past ~8KB of hash
the page offers download plus file-drop instead of any link. No backend,
no tracking.

## Wall-clock on WASM (G-003 delta)

`Instant` aborts on wasm32-unknown-unknown, so in-module wall-clock budgets
degrade to disabled there (`Budget::wall_supported()` is false; count,
depth, array, key, and value-size budgets still bound execution, and regex
matching is linear-time). Hosts must not promise in-module wall-clock
enforcement: run evaluation in a Worker and terminate on timeout. Never
`await` an unbounded eval on the UI thread.

`active_alloc(size)` is capped at 67108864 bytes (64 MiB); larger requests return null instead of allocating.


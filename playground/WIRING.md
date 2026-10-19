# Playground wiring contract (L-002 MVP, implemented by W-002)

`playground.js` talks ONLY to `window.ActiveAPI`. The active-wasm JS/TS shim
must provide exactly this shape; no evaluation logic may live in page JS.

```ts
window.ActiveAPI = {
  // Evaluate Active source. Never throws for language errors: they come
  // back as { ok: false, json: "", diagnostics: "<code+span lines>" }.
  // Host-side failures (OOM, engine missing) may throw Error.
  evaluate(source: string): { ok: boolean; json: string; diagnostics: string };
}
```

- `json` is the `active eval` JSON with secrets redacted (same bytes the CLI prints).
- `diagnostics` is one `file:line:col: [CODE] message` line per diagnostic, same order as CLI stderr.
- Secrets: the shim must never log source or values; redaction happens inside the WASM module.

## Local-dev loop (until B-001 wires the released artifact)

1. Build the workspace WASM module (W-001) for `wasm32-unknown-unknown`.
2. Serve this directory statically (`python -m http.server`); `fetch()` needs http, not `file://`.
3. Load `index.html`; without the engine the page says "Engine not loaded" (honest empty state, never faked output).

## Share links (G-019 decision)

MVP links carry raw encoded source (`#s=...`) and stay under ~2KB. The WASM
build adds lz-string-class compression; past URL length limits the page offers
download plus file-drop instead of a link. No backend, no tracking.

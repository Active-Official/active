# Active JavaScript/TypeScript Binding — WASM-first (CHECKLIST 1.007-1.008)

Thin agent-loop wrapper (`active.js`) over the hand-written shim
(`active-wasm/js/active-shim.js`): opaque module handle, ABI major gate,
Allocator Model A. Zero dependencies; runs on Node 20+, Deno, Bun, browsers.

## Decision (DERIVED, owner authority)

WASM-first. No N-API addon until profiling justifies one. The npm name is
**`active-lang`** on the public npm registry (unscoped `active` belongs to another owner; @active-lang/active` held as scoped fallback). Install with `npm install active-lang` (if `latest` still resolves a staged placeholder instead of the current version, the release is still clearing npm review — retry `npm view active-lang version` later). TypeScript types ship in `active.d.ts`,
mirroring `active.js` exactly.

## API

```js
const { load } = require("./bindings/js/active.js");
const m = await load();                 // or load(pathToWasm)
m.check(source);                        // [{code, message}], [] when clean
m.evaluate(source);                     // redacted object, throws on refusal
m.close();
```

## Tests

`test_active.js` drives the shipped module: eval value, clean check,
units refusal with T001, structured spans plus severity on diagnostics,
`validate`/`check` agreement (codes, spans, severity, message), secret
redaction with no raw leak, determinism.
Run from the repo root: `node bindings/js/test_active.js` (prints `js-ok`).

Type shape is gated by `tsc --noEmit` against the committed config
(`bindings/js/tsconfig.json`, which typechecks `active.d.ts` plus the
`active-types.test.ts` shape test that binds every exported member):

```bash
npx --yes -p typescript@5.6 tsc --noEmit -p bindings/js/tsconfig.json
```

Keep `active.d.ts` mirroring `active.js` exactly; the shape test fails the
moment they drift.

## Load-once guidance

Load the WASM module **once per process** and reuse the module across
calls: `load()` instantiates the linear-memory evaluator, and repeated
loads pay the full instantiation cost with no benefit. Pass an explicit
path only to pin a different build (e.g. a locally built `active_wasm.wasm`).

## Ownership and lifetime (normative for this binding)

- **Opaque handles:** a loaded module owns exactly one WASM instance. One module per task; do not share a module across concurrent async work that interleaves calls without external ordering. Close it when done; using a closed module throws.
- **Allocator Model A:** every allocation the module makes is freed inside the module through the official `active_free` export (handled by the shim). Host code never frees module memory.
- **ABI gate:** `load()` reads the module's ABI version and rejects a stale or foreign `.wasm` before returning.
- **Error mapping:** evaluation refusal throws (never a bare null or an empty object); a missing module or malformed envelope throws instead of returning plausible data.
- **Close discipline:** call `m.close()` when finished; the guard throws on any later use.

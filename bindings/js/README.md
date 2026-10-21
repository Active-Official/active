# Active JavaScript/TypeScript Binding — WASM-first (CHECKLIST 1.007-1.008)

Thin agent-loop wrapper (`active.js`) over the hand-written shim
(`active-wasm/js/active-shim.js`): opaque module handle, ABI major gate,
Allocator Model A. Zero dependencies; runs on Node 20+, Deno, Bun, browsers.

## Decision (DERIVED, owner authority)

WASM-first. No N-API addon until profiling justifies one. The npm name
`@active-lang/active` is reserved for the Phase 3 packaging step (3.004)
and is not claimed now.

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
units refusal with T001, secret redaction with no raw leak, determinism.
Run from the repo root: `node bindings/js/test_active.js` (prints `js-ok`).

# active-wasm

The Active evaluator for WebAssembly hosts: browsers, Node, Deno, Bun,
Cloudflare Workers, and pure-Go via wazero. Same guarantees as the CLI
(deterministic, budgeted, secret-redacting) through a linear-memory JSON
envelope: the host writes source bytes, calls `active_eval_json`, reads
the envelope, frees both sides.

**Active is the configuration language that refuses to lie.** What you
write is what runs. Units have dimensions; secrets have taint; budgets
always hold.

## Wall clock: the host owns the timeout

On wasm32 there is no monotonic clock, so `active_eval_json` runs without a
wall-clock deadline. The count, depth, array, key, and value-size budgets
still bind (a pathological document fails closed with an `X0xx` code), but
nothing stops a document that is legal and merely slow. Hosts MUST enforce
their own timeout.

The envelope reports `wall_supported` (`false` on wasm32, `true` on a
native build of the same code) on EVERY response, so a host can detect the
degradation instead of discovering it under load. Check it once at startup:

```js
const env = JSON.parse(activeEval(source));
if (!env.wall_supported) {
  // No in-module deadline: bound the call at the host level.
}
```

Worker-terminate pattern (Cloudflare Workers, Node worker_threads, browser
Workers): run the evaluation in a worker and terminate it when the budget
elapses. A worker that is terminated cannot leak memory or CPU back into the
caller, and the caller sees a clean timeout instead of a hung process.

```js
// Cloudflare Worker: evaluate in a child worker with a hard wall clock.
export default {
  async fetch(request, env, ctx) {
    const worker = new Worker(new URL("./eval-worker.js", import.meta.url).href);
    const timer = setTimeout(() => worker.terminate(), 2_000);
    try {
      const result = await new Promise((resolve, reject) => {
        worker.onmessage = (event) => resolve(event.data);
        worker.onerror = reject;
        worker.postMessage({ source: await request.text() });
      });
      return new Response(result);
    } finally {
      clearTimeout(timer);
      worker.terminate();
    }
  },
};
```

The same pattern applies to `worker_threads` in Node (`worker.terminate()`
after a `setTimeout`) and to browser Workers. Native builds do not need it:
the evaluator's own `max_wall_ms` backstop fires there.


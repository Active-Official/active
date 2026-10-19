# active-wasm

The Active evaluator for WebAssembly hosts: browsers, Node, Deno, Bun,
Cloudflare Workers, and pure-Go via wazero. Same guarantees as the CLI
(deterministic, budgeted, secret-redacting) through a linear-memory JSON
envelope: the host writes source bytes, calls `active_eval_json`, reads
the envelope, frees both sides.

**Active is the configuration language that refuses to lie.** What you
write is what runs. Units have dimensions; secrets have taint; budgets
always hold.

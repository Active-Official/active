/* active-wasm JS shim (W-002): implements window.ActiveAPI over the raw
 * module exports (alloc/write/eval/read/free). Zero dependencies, no
 * bundler, no generated glue: the .wasm has no imports, so plain
 * WebAssembly.instantiate suffices in browsers, Node, Deno, and Bun.
 * TypeScript declarations live beside this file (active-shim.d.ts).
 *
 * Wall-clock note (G-003): in-module wall budgets degrade to disabled on
 * wasm32 (Budget.wall_supported() is false). Hosts needing hard timeouts
 * run evaluation in a Worker and terminate on timeout; never await an
 * unbounded eval on the UI thread.
 */
(() => {
	async function load(source) {
		// `source` is a Response, URL string, or ArrayBuffer with the .wasm bytes.
		const bytes =
			source instanceof ArrayBuffer
				? source
				: await (typeof source === "string"
						? fetch(source).then((r) => r.arrayBuffer())
						: source.arrayBuffer());
		const { instance } = await WebAssembly.instantiate(bytes, {});
		const ex = instance.exports;
		if (
			typeof ex.active_abi_version !== "function" ||
			ex.active_abi_version() !== 1
		) {
			throw new Error("active-wasm ABI mismatch: expected 1");
		}
		const mem = () => new Uint8Array(ex.memory.buffer);
		const read = (ptr, len) =>
			new TextDecoder().decode(mem().slice(ptr, ptr + len));

		function evaluate(input) {
			const src = new TextEncoder().encode(input);
			const sp = ex.active_alloc(src.length);
			mem().set(src, sp);
			const slot = ex.active_alloc(4);
			const out = ex.active_eval_json(sp, src.length, slot);
			const n = new DataView(ex.memory.buffer, slot, 4).getUint32(0, true);
			let env;
			try {
				env = JSON.parse(read(out, n));
			} finally {
				ex.active_free(sp, src.length);
				ex.active_free(out, n);
				ex.active_free(slot, 4);
			}
			return {
				ok: env.ok === true,
				json: JSON.stringify(env.json),
				diagnostics: String(env.diagnostics || ""),
			};
		}

		const api = { evaluate, abiVersion: 1 };
		if (typeof window !== "undefined") window.ActiveAPI = api;
		return api;
	}

	if (typeof module !== "undefined" && module.exports)
		module.exports = { load };
	else window.ActiveShim = { load };
})();

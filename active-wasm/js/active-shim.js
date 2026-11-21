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

		function evaluateWith(input, pure) {
			const src = new TextEncoder().encode(input);
			const sp = ex.active_alloc(src.length);
			// A request over the 64 MiB transport cap returns null; without
			// this check `mem().set` would clobber the zero page and the
			// length slot would read garbage. Fail loudly instead.
			if (!sp) {
				throw new Error("active-wasm: input allocation failed (64 MiB cap)");
			}
			mem().set(src, sp);
			const slot = ex.active_alloc(4);
			if (!slot) {
				ex.active_free(sp, src.length);
				throw new Error("active-wasm: envelope length allocation failed");
			}
			const out = pure
				? ex.active_eval_json_pure(sp, src.length, slot)
				: ex.active_eval_json(sp, src.length, slot);
			if (!out) {
				ex.active_free(sp, src.length);
				ex.active_free(slot, 4);
				throw new Error("active-wasm: evaluation returned no envelope");
			}
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
				// Passed through verbatim when present, ABSENT otherwise:
				// the bindings fail closed on a missing items array, so the
				// shim must never substitute a default here (a stale module
				// would otherwise read as a false clean).
				...(env.items !== undefined ? { items: env.items } : {}),
			};
		}

		function evaluate(input) {
			return evaluateWith(input, false);
		}

		// L-P05: refuse env reads, unpinned includes, and profile overrides
		// (E047). A module without the export is stale; fail loudly rather
		// than silently evaluating impurely.
		function evaluatePure(input) {
			if (typeof ex.active_eval_json_pure !== "function") {
				throw new Error(
					"active-wasm: pure mode requires a module built with L-P05 (active_eval_json_pure)",
				);
			}
			return evaluateWith(input, true);
		}

		const api = { evaluate, evaluatePure, abiVersion: 1 };
		if (typeof window !== "undefined") window.ActiveAPI = api;
		return api;
	}

	if (typeof module !== "undefined" && module.exports)
		module.exports = { load };
	else window.ActiveShim = { load };
})();

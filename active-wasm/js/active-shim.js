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
						? fetch(source).then((r) => {
								// A 404 page is HTML, not a module; without this
								// check the fallback source never fires and the
								// failure reads as a WebAssembly parse error.
								if (!r.ok)
									throw new Error(
										"active-wasm: HTTP " + r.status + " for " + source,
									);
								return r.arrayBuffer();
							})
						: source.ok === false
							? Promise.reject(new Error("active-wasm: HTTP " + source.status))
							: typeof source.arrayBuffer !== "function"
								? Promise.reject(
										new Error(
											"active-wasm: source has no bytes (expected a URL, ArrayBuffer, or Response)",
										),
									)
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

		// L-P14: the result record is the host's copy; freeze it (and the
		// nested diagnostic items) so an accidental mutation fails instead
		// of silently corrupting a run. The language's hash is computed
		// before this copy exists; a host that copies and mutates still
		// holds a different value, per the determinism contract.
		function deepFreeze(value) {
			if (value && typeof value === "object" && !Object.isFrozen(value)) {
				Object.freeze(value);
				// Own data properties only: never trigger a getter and never
				// walk an inherited prototype. JSON.parse results are plain
				// data, so this covers every nested span and item.
				for (const key of Object.getOwnPropertyNames(value)) {
					const descriptor = Object.getOwnPropertyDescriptor(value, key);
					if (descriptor && "value" in descriptor) deepFreeze(descriptor.value);
				}
			}
			return value;
		}

		// Bound before allocating the UTF-8 copy: a huge host string must
		// not OOM the tab before the wasm transport cap can reject it.
		// The allocator still enforces the exact 64 MiB byte cap.
		function encodeInput(input) {
			if (typeof input !== "string") {
				throw new Error("active-wasm: input must be a string");
			}
			if (input.length > 16 * 1024 * 1024) {
				throw new Error("active-wasm: input exceeds the transport cap");
			}
			return new TextEncoder().encode(input);
		}

		function evaluateWith(input, pure) {
			const src = encodeInput(input);
			const sp = ex.active_alloc(src.length);
			// A request over the 64 MiB transport cap returns null; without
			// this check `mem().set` would clobber the zero page and the
			// length slot would read garbage. Fail loudly instead.
			if (!sp) {
				throw new Error("active-wasm: input allocation failed (64 MiB cap)");
			}
			let slot = 0;
			let out = 0;
			let n = 0;
			try {
				// A growth between calls can detach the old view; `set`
				// throws then, and the finally below still frees both
				// transient allocations.
				mem().set(src, sp);
				slot = ex.active_alloc(4);
				if (!slot) {
					throw new Error("active-wasm: envelope length allocation failed");
				}
				out = pure
					? ex.active_eval_json_pure(sp, src.length, slot)
					: ex.active_eval_json(sp, src.length, slot);
				if (!out) {
					throw new Error("active-wasm: evaluation returned no envelope");
				}
				n = new DataView(ex.memory.buffer, slot, 4).getUint32(0, true);
				// The length is host-readable memory, so a buggy or hostile
				// module could hand back a huge value; never slice past the
				// transport cap or allocate a string that large.
				if (!Number.isInteger(n) || n > 67108864) {
					throw new Error("active-wasm: envelope length out of bounds");
				}
				const env = JSON.parse(read(out, n));
				return deepFreeze({
					ok: env.ok === true,
					json: JSON.stringify(env.json),
					diagnostics: String(env.diagnostics || ""),
					// Passed through verbatim when present, ABSENT otherwise:
					// the bindings fail closed on a missing items array, so the
					// shim must never substitute a default here (a stale module
					// would otherwise read as a false clean).
					...(env.items !== undefined ? { items: env.items } : {}),
				});
			} finally {
				if (out) ex.active_free(out, n);
				if (slot) ex.active_free(slot, 4);
				ex.active_free(sp, src.length);
			}
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

		// L-P47: BLAKE3 hex digest of the source bytes, so the playground
		// can verify a hash-addressed snippet before evaluating it. A
		// module without the export is stale; fail loudly.
		function hash(input) {
			if (typeof ex.active_blake3 !== "function") {
				throw new Error(
					"active-wasm: hashing requires a module built with L-P47 (active_blake3)",
				);
			}
			const src = encodeInput(input);
			const sp = ex.active_alloc(src.length);
			if (!sp) {
				throw new Error("active-wasm: input allocation failed (64 MiB cap)");
			}
			let out = 0;
			try {
				// A detached view after a growth makes `set` throw; the
				// finally still frees the input allocation.
				mem().set(src, sp);
				out = ex.active_blake3(sp, src.length);
				if (!out) {
					throw new Error("active-wasm: hash returned no digest");
				}
				let hex;
				try {
					hex = read(out, 64);
				} finally {
					ex.active_free(out, 64);
					out = 0;
				}
				if (!/^[0-9a-f]{64}$/.test(hex)) {
					throw new Error(
						"active-wasm: hash was not a 64-character hex digest",
					);
				}
				return hex;
			} finally {
				ex.active_free(sp, src.length);
				if (out) ex.active_free(out, 64);
			}
		}

		const api = Object.freeze({ evaluate, evaluatePure, hash, abiVersion: 1 });
		if (typeof window !== "undefined") window.ActiveAPI = api;
		return api;
	}

	if (typeof module !== "undefined" && module.exports)
		module.exports = { load };
	else window.ActiveShim = { load };
})();

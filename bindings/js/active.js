// Active JS/TS binding, WASM-first (CHECKLIST 1.007-1.008, PROPOSAL 2.1, 2.3).
//
// Thin agent-loop wrapper over the hand-written shim
// (active-wasm/js/active-shim.js): opaque module handle, ABI major gate,
// Allocator Model A (the module's active_free frees everything).
// Zero dependencies, runs on Node 20+, Deno, Bun, and browsers.
//
// Decision (DERIVED, owner authority): WASM-first via the shim. A native
// N-API addon stays an option if profiling ever justifies it; the package
// name `@active-lang/active` is reserved for the future npm publish
// (CHECKLIST 3.004), not claimed now.

const fs = require("fs");
const path = require("path");
const shim = require("../../active-wasm/js/active-shim.js");

function defaultWasmPath() {
	return path.join(
		__dirname,
		"..",
		"..",
		"target",
		"wasm32-unknown-unknown",
		"release",
		"active_wasm.wasm",
	);
}

async function load(wasmPath) {
	const file = wasmPath || defaultWasmPath();
	if (!fs.existsSync(file)) throw new Error("wasm module not found: " + file);
	const bytes = fs.readFileSync(file);
	const api = await shim.load(
		bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
	);
	return new Module(api);
}

class Module {
	constructor(api) {
		this._api = api;
		this._closed = false;
	}
	close() {
		this._closed = true;
	}
	_guard() {
		if (this._closed) throw new Error("module is closed");
	}

	check(source) {
		this._guard();
		const env = this._api.evaluate(source);
		if (!Array.isArray(env.items)) {
			throw new Error("envelope without items: rebuild active_wasm.wasm");
		}
		const text = env.diagnostics || "";
		return env.items.map((item) => {
			const span = item.span || null;
			return {
				code: item.code,
				message: text,
				line: span && span.line !== undefined ? span.line : null,
				column: span && span.column !== undefined ? span.column : null,
				end_line: span && span.end_line !== undefined ? span.end_line : null,
				end_column:
					span && span.end_column !== undefined ? span.end_column : null,
				severity: item.severity !== undefined ? item.severity : null,
			};
		});
	}

	evaluate(source) {
		this._guard();
		const env = this._api.evaluate(source);
		if (!env.ok) throw new Error("refused: " + env.diagnostics.slice(0, 200));
		return JSON.parse(env.json);
	}
}

module.exports = { load, Module };

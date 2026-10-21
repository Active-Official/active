// Active JS/TS binding, WASM-first (CHECKLIST 1.007-1.008, PROPOSAL 2.1, 2.3).
//
// Thin agent-loop wrapper over the hand-written shim
// (active-wasm/js/active-shim.js): opaque module handle, ABI major gate,
// Allocator Model A (the module's active_free frees everything).
// Zero dependencies, runs on Node 20+, Deno, Bun, and browsers.
//
// Decision (DERIVED, owner authority): WASM-first via the shim. A native
// N-API addon stays an option if profiling ever justifies it; the package
// name is decided as `@active-lang/active` (registry-verified free;
// publish-ready, awaiting an owner token to claim).

const fs = require("fs");
const path = require("path");
// The shim ships inside the npm package (copied by `npm run prepack`)
// and lives in active-wasm/js during repo development.
const shim = (() => {
	try {
		return require("./active-shim.js");
	} catch (e) {
		// Packaged layout first; fall back to the repo tree only when
		// the bundled shim is absent — never mask a real load error.
		if (e && e.code === "MODULE_NOT_FOUND") {
			return require("../../active-wasm/js/active-shim.js");
		}
		throw e;
	}
})();

function packageVersion() {
	try {
		return require("./package.json").version;
	} catch {
		return "1.4.0";
	}
}

function defaultWasmPath() {
	// Order: $ACTIVE_WASM_PATH, bundled active_wasm.wasm (placed by
	// packaging/fetch-wasm.py at pack time), in-repo build (development).
	if (process.env.ACTIVE_WASM_PATH) return process.env.ACTIVE_WASM_PATH;
	const bundled = path.join(__dirname, "active_wasm.wasm");
	if (fs.existsSync(bundled)) return bundled;
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
	if (!fs.existsSync(file))
		throw new Error(
			"wasm module not found: " +
				file +
				" (pass a path, set ACTIVE_WASM_PATH, or place active_wasm.wasm next to the package — see packaging/fetch-wasm.py or the release asset at https://github.com/Active-Official/active/releases/tag/v" +
				packageVersion() +
				")",
		);
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
		if (!env || typeof env !== "object") {
			throw new Error("envelope is not an object: rebuild active_wasm.wasm");
		}
		if (!Array.isArray(env.items)) {
			throw new Error("envelope without items: rebuild active_wasm.wasm");
		}
		const text = env.diagnostics || "";
		return env.items.map((item) => {
			if (!item || typeof item.code !== "string" || item.code === "") {
				// A codeless item is envelope corruption, not a diagnostic:
				// fail closed instead of emitting undefined codes. Mirrors
				// the Python binding's missing-code guard.
				throw new Error("envelope item without code: rebuild active_wasm.wasm");
			}
			const span = item.span || null;
			const full =
				span &&
				span.line !== undefined &&
				span.column !== undefined &&
				span.end_line !== undefined &&
				span.end_column !== undefined;
			return {
				code: item.code,
				// Per-item message from the core (redacted by construction),
				// falling back to the envelope text only when absent.
				message:
					typeof item.message === "string" && item.message !== ""
						? item.message
						: text,
				line: full ? span.line : null,
				column: full ? span.column : null,
				end_line: full ? span.end_line : null,
				end_column: full ? span.end_column : null,
				severity: item.severity !== undefined ? item.severity : null,
			};
		});
	}

	validate(source) {
		// Validation step of the agent loop: same hermetic verdict as
		// check(), kept as a separate entry point so portable agent code
		// can call validate() on every Tier-1 binding. Budgets are the
		// engine's hard shipped defaults (fail-closed); per-call custom
		// budgets belong to the C ABI surface.
		return this.check(source);
	}

	evaluate(source) {
		this._guard();
		const env = this._api.evaluate(source);
		if (!env || typeof env !== "object") {
			throw new Error("refused: envelope is not an object");
		}
		if (!env.ok)
			throw new Error(
				"refused: " + String(env.diagnostics || "").slice(0, 200),
			);
		// The shim serializes the JSON payload to a string; accept an
		// already-decoded object too (other shims), and fail closed on
		// anything else instead of returning plausible-looking data.
		let value = env.json;
		if (typeof value === "string") {
			try {
				value = JSON.parse(value);
			} catch (e) {
				throw new Error("refused: envelope JSON is not valid JSON");
			}
		}
		if (value === null || typeof value !== "object" || Array.isArray(value)) {
			throw new Error("refused: envelope without a JSON object value");
		}
		return JSON.parse(JSON.stringify(value));
	}
}

module.exports = { load, Module };

/* WASM conformance runner (C-003 non-Rust embedding): executes
 * conformance/manifest.toml against the active-wasm shim. Codes compare
 * strictly; spans stay advisory (G-017); manifest [[pins]] record as pins,
 * never failures. Usage: node active-wasm/run-conformance.js [wasm-path]
 */
const fs = require("fs");
const path = require("path");

const ROOT = __dirname + "/..";
const SHIM = require("./js/active-shim.js");

function parseManifest(text) {
	const cases = [];
	const pins = {};
	let cur = null;
	let section = null;
	for (const line of text.split("\n")) {
		const t = line.trim();
		if (t === "[[cases]]") {
			cur = {};
			section = "case";
			cases.push(cur);
			continue;
		}
		if (t === "[[pins]]") {
			cur = {};
			section = "pin";
			continue;
		}
		const m = t.match(/^(\w+)\s*=\s*"(.*)"$/);
		if (m && cur) cur[m[1]] = m[2];
		if (section === "pin" && cur && cur.id && cur.reason)
			pins[cur.id] = cur.reason;
	}
	return { cases, pins };
}

function codesOf(text) {
	const out = [];
	const re = /\b([A-Z][0-9]{3})\b/g;
	let m;
	while ((m = re.exec(text)) !== null) {
		if (!out.includes(m[1])) out.push(m[1]);
	}
	return out;
}

async function main() {
	const wasmPath =
		process.argv[2] ||
		ROOT + "/target/wasm32-unknown-unknown/release/active_wasm.wasm";
	const manifest = parseManifest(
		fs.readFileSync(ROOT + "/conformance/manifest.toml", "utf8"),
	);
	const bytes = fs.readFileSync(wasmPath);
	const api = await SHIM.load(
		bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
	);
	let pass = 0;
	let pins = 0;
	for (const c of manifest.cases) {
		if (manifest.pins[c.id]) {
			console.log(c.id, "PINNED:", manifest.pins[c.id]);
			pins++;
			continue;
		}
		const src = fs.readFileSync(path.join(ROOT, "conformance", c.file), "utf8");
		const r = api.evaluate(src);
		const got = JSON.parse(r.json || "null");
		const codes = codesOf(r.diagnostics);
		// Deep value comparison: object key order is not semantics, and a
		// formatter or a different serializer may legitimately reorder keys
		// (the differential gate compares parsed values for the same
		// reason). Arrays stay order-sensitive.
		const deepEqual = (a, b) => {
			if (a === b) return true;
			if (typeof a !== typeof b || a === null || b === null) return false;
			if (Array.isArray(a) || Array.isArray(b)) {
				if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length)
					return false;
				return a.every((item, i) => deepEqual(item, b[i]));
			}
			if (typeof a === "object") {
				const ka = Object.keys(a).sort();
				const kb = Object.keys(b).sort();
				if (ka.length !== kb.length || ka.some((k, i) => k !== kb[i]))
					return false;
				return ka.every((k) => deepEqual(a[k], b[k]));
			}
			return false;
		};
		let ok = false;
		if (c.kind === "clean") {
			const want = JSON.parse(
				fs.readFileSync(
					path.join(ROOT, "conformance/expected", c.id + ".json"),
					"utf8",
				),
			);
			ok = r.ok && codes.length === 0 && deepEqual(got, want);
		} else if (c.kind === "diagnosed") {
			const want = JSON.parse(
				fs.readFileSync(
					path.join(ROOT, "conformance/expected", c.id + ".json"),
					"utf8",
				),
			);
			const wantCodes = JSON.parse(
				fs.readFileSync(
					path.join(ROOT, "conformance/errors", c.id + ".json"),
					"utf8",
				),
			).codes;
			ok =
				deepEqual(got, want) &&
				JSON.stringify(codes) === JSON.stringify(wantCodes);
		} else {
			const wantCodes = JSON.parse(
				fs.readFileSync(
					path.join(ROOT, "conformance/errors", c.id + ".json"),
					"utf8",
				),
			).codes;
			ok = JSON.stringify(codes) === JSON.stringify(wantCodes);
		}
		console.log(c.id, ok ? "PASS" : "FAIL", "codes=" + JSON.stringify(codes));
		if (ok) pass++;
		else process.exitCode = 1;
	}
	console.log(
		pass + "/" + manifest.cases.length + " pass, " + pins + " pinned",
	);
}

main().catch((e) => {
	console.error("WASM-CONF-FAIL:", e.message);
	process.exit(1);
});

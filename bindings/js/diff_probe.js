// Differential probe for the shared harness (CHECKLIST 3.001).
// Loads the shipped wrapper over the shipped WASM module, runs one
// source file through check + evaluate, and prints a single JSON line:
// {codes:[...], items:[{code,line,column,end_line,end_column,severity}],
//  refused:bool, value:object|null}. Run from the repo root:
// node bindings/js/diff_probe.js [wasmPath|-] <srcFile>
const fs = require("fs");
const { load } = require("./active.js");

(async () => {
	const wasmArg = process.argv[2];
	const srcFile = process.argv[3];
	const mod = await load(wasmArg === "-" || !wasmArg ? undefined : wasmArg);
	const src = fs.readFileSync(srcFile, "utf8");
	const diags = mod.check(src).map((d) => ({
		code: d.code,
		line: d.line === undefined ? null : d.line,
		column: d.column === undefined ? null : d.column,
		end_line: d.end_line === undefined ? null : d.end_line,
		end_column: d.end_column === undefined ? null : d.end_column,
		severity: d.severity === undefined ? null : d.severity,
	}));
	let value = null;
	let refused = false;
	try {
		value = mod.evaluate(src);
	} catch (e) {
		refused = true;
	}
	console.log(
		JSON.stringify({
			codes: diags.map((d) => d.code),
			items: diags,
			refused,
			value,
		}),
	);
})().catch((e) => {
	console.error("diffprobe-FAIL: " + e.message);
	process.exit(1);
});

// Conformance-facing tests for the JS binding (CHECKLIST 1.008).
// Run from the repo root: node bindings/js/test_active.js
// Exit code 0 with "js-ok" on success (the harness tally).

const assert = require("assert");
const { load } = require("./active.js");

(async () => {
	const m = await load();
	assert.deepStrictEqual(m.evaluate("port = 8080\n"), { port: 8080 });
	assert.deepStrictEqual(m.check('country = "NO"\n'), []);
	const diags = m.check("timeout = 5s + 512MiB\n");
	assert.ok(
		diags.some((d) => d.code === "T001"),
		"T001 expected",
	);
	assert.strictEqual(diags.length, 1);
	assert.deepStrictEqual(
		[diags[0].line, diags[0].column, diags[0].end_line, diags[0].end_column],
		[1, 14, 1, 15],
	);
	assert.strictEqual(diags[0].severity, "error");

	// validate() is the loop's validation entry point: same verdict as
	// check() on every input, clean or not.
	for (const src of [
		"port = 8080\n",
		"timeout = 5s + 512MiB\n",
		'token = secret("hunter2")\n[out]\npass = token\n',
	]) {
		const shape = (d) => [
			d.code,
			d.line,
			d.column,
			d.end_line,
			d.end_column,
			d.severity,
			d.message,
		];
		const a = m.check(src).map(shape);
		const b = m.validate(src).map(shape);
		assert.deepStrictEqual(
			b,
			a,
			`validate/check diverge on ${JSON.stringify(src)}`,
		);
	}
	assert.throws(() => m.evaluate("timeout = 5s + 512MiB\n"), /refused/);
	// Repair entry point: the verdict stays a refusal (ok:false) while the
	// engine's best-effort tree and diagnostics remain available, and a
	// parse failure carries no tree at all.
	const t003 =
		"schema = {\n" +
		'  contact = String(format = "email"),\n' +
		"}\n" +
		'contact = "not-an-email"\n';
	const repaired = m.evaluateResilient(t003);
	assert.strictEqual(repaired.ok, false);
	assert.ok(repaired.diagnostics.some((d) => d.code === "T003"));
	assert.ok(repaired.value !== null && typeof repaired.value === "object");
	const clean = m.evaluateResilient("port = 8080\n");
	assert.strictEqual(clean.ok, true);
	assert.deepStrictEqual(clean.value, { port: 8080 });
	assert.deepStrictEqual(clean.diagnostics, []);
	const broken = m.evaluateResilient("[unclosed\n");
	assert.strictEqual(broken.ok, false);
	assert.strictEqual(broken.value, null);
	assert.ok(broken.diagnostics.length > 0);
	const v = m.evaluate('token = secret("hunter2")\n[out]\npass = token\n');
	// Parent-scope resolution (spec §2.4) resolves `pass` to the root
	// secret, so it redacts instead of reading null.
	assert.deepStrictEqual(v, {
		token: "***SECRET***",
		out: { pass: "***SECRET***" },
	});
	assert.ok(!JSON.stringify(v).includes("hunter2"));
	const src = "let x = 1 + 2\n[out]\nv = x * 3\n";
	assert.deepStrictEqual(m.evaluate(src), m.evaluate(src));
	m.close();

	// WASM resolution: $ACTIVE_WASM_PATH wins; bogus paths fail
	// closed with acquisition guidance. The release-path phase needs a
	// built release module; without it the phase is SKIPPED with a
	// visible note (the harness tally stays honest: the note is the
	// record that the phase did not run).
	const savedWasmPath = process.env.ACTIVE_WASM_PATH;
	const releaseWasm = require("path").join(
		__dirname,
		"..",
		"..",
		"target",
		"wasm32-unknown-unknown",
		"release",
		"active_wasm.wasm",
	);
	if (require("fs").existsSync(releaseWasm)) {
		process.env.ACTIVE_WASM_PATH = releaseWasm;
		try {
			const m2 = await load();
			assert.deepStrictEqual(m2.evaluate("port = 8080\n"), { port: 8080 });
			m2.close();
		} finally {
			if (savedWasmPath === undefined) delete process.env.ACTIVE_WASM_PATH;
			else process.env.ACTIVE_WASM_PATH = savedWasmPath;
		}
	} else {
		console.log("js-note: release wasm not built, skipping release-path phase");
	}

	// L-P35: the capability-gated evaluate path through the shipped shim
	// against the release module. Empty held authority must refuse with
	// E049; a granting set must evaluate. The loop also watches process
	// RSS: every transient allocation a call takes is released in its
	// finally, so a long run must not grow without bound (a shim that
	// drops those frees grows by hundreds of MiB over the same loop).
	if (require("fs").existsSync(releaseWasm)) {
		const shim = require("../../active-wasm/js/active-shim.js");
		const api = await shim.load(require("fs").readFileSync(releaseWasm));
		const denied = api.evaluateWithCapabilities("p = env.SECRET\n", "");
		assert.strictEqual(denied.ok, false, "empty authority must refuse");
		assert.ok(
			denied.diagnostics.includes("E049"),
			"empty authority must report E049",
		);
		const allowed = api.evaluateWithCapabilities(
			"p = env.SECRET\n",
			"evaluate,read-secrets",
		);
		assert.strictEqual(allowed.ok, true, "granting authority must evaluate");
		assert.deepStrictEqual(JSON.parse(allowed.json), { p: null });
		// A ~30 KB source makes a single leaked buffer per call plain.
		const big = 'v = "' + "a".repeat(30000) + '"\n';
		const caps = "evaluate,read-secrets";
		for (let i = 0; i < 200; i++) api.evaluateWithCapabilities(big, caps);
		const before = process.memoryUsage().external;
		for (let i = 0; i < 2000; i++) api.evaluateWithCapabilities(big, caps);
		const grew = process.memoryUsage().external - before;
		assert.ok(
			grew < 48 * 1024 * 1024,
			`transient allocations must be freed (external grew ${(grew / 1048576).toFixed(1)} MiB)`,
		);
	} else {
		console.log("js-note: release wasm not built, skipping capability phase");
	}

	await assert.rejects(
		load("/nonexistent/active_wasm.wasm"),
		/ACTIVE_WASM_PATH/,
	);
	console.log("js-ok");
})().catch((e) => {
	console.error("js-FAIL:", e.message);
	process.exit(1);
});

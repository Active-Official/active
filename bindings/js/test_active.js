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
		const shape = (d) => [d.code, d.line, d.column, d.end_line, d.end_column, d.severity, d.message];
		const a = m.check(src).map(shape);
		const b = m.validate(src).map(shape);
		assert.deepStrictEqual(b, a, `validate/check diverge on ${JSON.stringify(src)}`);
	}
	assert.throws(() => m.evaluate("timeout = 5s + 512MiB\n"), /refused/);
	const v = m.evaluate('token = secret("hunter2")\n[out]\npass = token\n');
	assert.deepStrictEqual(v, { token: "***SECRET***", out: { pass: null } });
	assert.ok(!JSON.stringify(v).includes("hunter2"));
	const src = "let x = 1 + 2\n[out]\nv = x * 3\n";
	assert.deepStrictEqual(m.evaluate(src), m.evaluate(src));
	m.close();
	console.log("js-ok");
})().catch((e) => {
	console.error("js-FAIL:", e.message);
	process.exit(1);
});

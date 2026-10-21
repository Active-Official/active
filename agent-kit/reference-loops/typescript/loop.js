// Reference agent loop in TypeScript (CHECKLIST 2.020, PROPOSAL section 4).
//
// Same contract as the Python loop: check (diagnose) -> resilient eval
// (repair) -> re-emit via the `active import` CLI -> check again
// (validate) -> final redacted eval. Budgeted iterations, stuck
// detection, code-only secret-safe logs. Run from the repo root:
// node agent-kit/reference-loops/typescript/loop.js <candidate.active>

const fs = require("fs");
const os = require("os");
const path = require("path");
const { execFileSync } = require("child_process");
const { load } = require("../../../bindings/js/active.js");

const ROOT = path.join(__dirname, "..", "..", "..");

function reemit(value) {
	const dir = fs.mkdtempSync(path.join(os.tmpdir(), "active-loop-"));
	const tmp = path.join(dir, "value.json");
	fs.writeFileSync(tmp, JSON.stringify(value), { mode: 0o600 });
	try {
		return execFileSync(
			"cargo",
			["run", "-q", "-p", "active-cli", "--", "import", tmp, "--from", "json"],
			{
				cwd: ROOT,
				encoding: "utf8",
				timeout: 120000,
				maxBuffer: 16 * 1024 * 1024,
			},
		);
	} finally {
		try {
			fs.unlinkSync(tmp);
			fs.rmdirSync(dir);
		} catch {
			/* best effort */
		}
	}
}

async function run(source, maxIters) {
	const mod = await load();
	const log = [];
	const seen = new Set();
	let current = source;
	try {
		for (let i = 1; i <= maxIters; i++) {
			const diags = mod.check(current);
			const codes = [...new Set(diags.map((d) => d.code))].sort();
			const entry = { iter: i, codes };
			log.push(entry);
			if (codes.length === 0) return [mod.evaluate(current), log];
			const key = codes.join(",");
			if (seen.has(key)) {
				entry.stuck = true;
				break;
			}
			seen.add(key);
			let value;
			try {
				value = mod.evaluate(current);
			} catch (e) {
				entry.unrepairable = true;
				break;
			}
			entry.repaired_by_resilient = true;
			current = reemit(value);
		}
		throw new Error(
			`unrepaired after ${log.length} iterations: ${JSON.stringify(log[log.length - 1])}`,
		);
	} finally {
		mod.close();
	}
}

(async () => {
	const file = process.argv[2];
	const maxIters = parseInt(
		(process.argv.find((a) => a.startsWith("--max-iters=")) || "=5").split(
			"=",
		)[1],
		10,
	);
	if (!file) {
		console.error("usage: loop.js <candidate.active> [--max-iters=N]");
		process.exit(2);
	}
	if (!Number.isInteger(maxIters) || maxIters < 1) {
		console.error("max-iters must be a positive integer");
		process.exit(2);
	}
	try {
		const [value, log] = await run(fs.readFileSync(file, "utf8"), maxIters);
		console.log(JSON.stringify({ value, log }, null, 2));
	} catch (e) {
		console.error(`FAILED: ${e.message}`);
		process.exit(1);
	}
})();

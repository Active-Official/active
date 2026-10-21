// prepack: copy the hand-written WASM shim plus the license texts next
// to the package entry points so the published tarball is
// self-contained. In-repo layout keeps the single sources of truth at
// active-wasm/js/active-shim.js and the repo-root LICENSE-* files;
// active.js resolves the local copy first and falls back to the repo
// tree only when the copy is absent (development).
const fs = require("fs");
const path = require("path");

const pairs = [
	["..", "..", "..", "active-wasm", "js", "active-shim.js", "active-shim.js"],
	["..", "..", "..", "LICENSE-MIT", "LICENSE-MIT"],
	["..", "..", "..", "LICENSE-APACHE", "LICENSE-APACHE"],
];
for (const parts of pairs) {
	const dstName = parts[parts.length - 1];
	const src = path.join(__dirname, ...parts.slice(0, -1));
	const dst = path.join(__dirname, "..", dstName);
	if (!fs.existsSync(src)) {
		console.error(
			`packaging file missing: ${src} — run from the repo checkout`,
		);
		process.exit(1);
	}
	fs.copyFileSync(src, dst);
	console.log(`copied ${dstName} for pack`);
}

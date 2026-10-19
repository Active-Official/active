/* Active Playground shell (L-002 MVP, L-017 sharing).
 *
 * Talks ONLY to window.ActiveAPI, the contract documented in WIRING.md and
 * implemented by the active-wasm JS/TS shim (W-002). No evaluation logic
 * lives here: when the engine is absent the page says so instead of faking.
 *
 * Share links: `#c=` carries deflate-raw-compressed source (platform
 * CompressionStream, base64url) when available, else `#s=` raw encoded
 * source. Past URL length limits the page offers download plus file-drop
 * instead of a link. No backend, no tracking.
 */
(() => {
	var source = document.getElementById("source");
	var valueBox = document.getElementById("value");
	var diagBox = document.getElementById("diags");
	var picker = document.getElementById("examples");

	var EXAMPLES = [
		"norway-contrast",
		"units-mismatch",
		"secret-interp-refusal",
		"schema-failure",
	];

	function setStatus(text, cls) {
		valueBox.textContent = text;
		valueBox.className = cls || "";
	}

	function boot() {
		EXAMPLES.forEach((name) => {
			var opt = document.createElement("option");
			opt.value = name;
			opt.textContent = name;
			picker.appendChild(opt);
		});
		picker.addEventListener("change", loadExample);
		document.getElementById("eval").addEventListener("click", evaluate);
		document
			.getElementById("download")
			.addEventListener("click", downloadSource);
		document.getElementById("dropfile").addEventListener("change", (e) => {
			if (e.target.files.length > 0) dropFile(e.target.files[0]);
		});
		loadFromHash() || loadExample();
		if (!window.ActiveAPI) {
			setStatus(
				"Engine not loaded: build active-wasm (W-001) and serve this directory.",
				"",
			);
			diagBox.textContent = "See WIRING.md for the local-dev loop.";
		}
	}

	function loadExample() {
		var name = picker.value || EXAMPLES[0];
		fetch("examples/" + name + ".active")
			.then((r) => r.text())
			.then((t) => {
				source.value = t;
			})
			.catch(() => {
				source.value = "# could not load " + name;
			});
	}

	function loadFromHash() {
		if (location.hash.startsWith("#c=")) return loadCompressed();
		if (!location.hash.startsWith("#s=")) return false;
		try {
			source.value = decodeURIComponent(location.hash.slice(3));
			return true;
		} catch (e) {
			return false;
		}
	}

	async function loadCompressed() {
		try {
			const bytes = fromB64url(location.hash.slice(3));
			const ds = new DecompressionStream("deflate-raw");
			const writer = ds.writable.getWriter();
			writer.write(bytes);
			writer.close();
			const out = await new Response(ds.readable).arrayBuffer();
			source.value = new TextDecoder().decode(out);
			return true;
		} catch (e) {
			return false;
		}
	}

	function toB64url(bytes) {
		let bin = "";
		const CHUNK = 8192;
		for (let i = 0; i < bytes.length; i += CHUNK) {
			bin += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
		}
		return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
	}

	function fromB64url(s) {
		const bin = atob(s.replace(/-/g, "+").replace(/_/g, "/"));
		const bytes = new Uint8Array(bin.length);
		for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
		return bytes;
	}

	async function shareSource(text) {
		// Prefer compressed links; fall back to raw links where the
		// platform lacks CompressionStream. Take whichever is shorter:
		// compression overhead loses on tiny documents. Past ~8KB of
		// hash, offer download plus file-drop instead of any link.
		const raw = "#s=" + encodeURIComponent(text);
		let best = raw;
		if (typeof CompressionStream !== "undefined") {
			try {
				const cs = new CompressionStream("deflate-raw");
				const writer = cs.writable.getWriter();
				writer.write(new TextEncoder().encode(text));
				writer.close();
				const buf = await new Response(cs.readable).arrayBuffer();
				const compressed = "#c=" + toB64url(new Uint8Array(buf));
				if (compressed.length < best.length) best = compressed;
			} catch (e) {
				/* keep raw */
			}
		}
		if (best.length < 8000) location.hash = best;
		else diagBox.textContent += "\n[Too large to link: use Download below.]";
	}

	function downloadSource() {
		const blob = new Blob([source.value], { type: "text/plain" });
		const a = document.createElement("a");
		a.href = URL.createObjectURL(blob);
		a.download = "shared.active";
		a.click();
		setTimeout(() => URL.revokeObjectURL(a.href), 5000);
	}

	function dropFile(file) {
		const reader = new FileReader();
		reader.onload = () => {
			source.value = String(reader.result || "");
		};
		reader.readAsText(file);
	}

	function evaluate() {
		if (!window.ActiveAPI) {
			diagBox.textContent = "Engine not loaded.";
			return;
		}
		try {
			var res = window.ActiveAPI.evaluate(source.value);
			setStatus(res.json, res.ok ? "ok" : "err");
			diagBox.textContent = res.diagnostics;
			if (res.ok) {
				shareSource(source.value);
			}
		} catch (e) {
			setStatus("Host error: " + e.message, "err");
		}
	}

	document.addEventListener("DOMContentLoaded", boot);
})();

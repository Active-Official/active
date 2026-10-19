/* Active Playground shell (L-002 MVP).
 *
 * Talks ONLY to window.ActiveAPI, the contract documented in WIRING.md and
 * implemented by the active-wasm JS/TS shim (W-002). No evaluation logic
 * lives here: when the engine is absent the page says so instead of faking.
 *
 * Share links: #s=<lz-compressed source> per G-019 (compression lands with
 * the WASM build; until then links carry raw encoded source under 2KB).
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
		if (!location.hash.startsWith("#s=")) return false;
		try {
			source.value = decodeURIComponent(location.hash.slice(3));
			return true;
		} catch (e) {
			return false;
		}
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
				location.hash = "#s=" + encodeURIComponent(source.value);
			}
		} catch (e) {
			setStatus("Host error: " + e.message, "err");
		}
	}

	document.addEventListener("DOMContentLoaded", boot);
})();

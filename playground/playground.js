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
	var shareLink = document.getElementById("sharelink");
	// L-P47: hash-addressed snippets share the engine's hash and a bounded
	// local cache; the result cache is per page session (an engine change
	// must not serve a stale result across sessions).
	var snippets = window.ActiveSnippets;
	var resultCache = snippets ? snippets.createResultCache() : null;
	var cachedApi = null;
	var SOURCE_LIMIT = snippets ? snippets.MAX_SOURCE_BYTES : 8192;
	// Base64/URI slack for a source at the cap; a longer fragment is
	// refused before it is decoded (a zip bomb must not allocate).
	var LINK_LIMIT = 12000;
	var refused = false;

	// A sandboxed or privacy-blocked iframe can throw on the getter itself;
	// the cache is optional, so a null store simply means "no cache".
	function localStore() {
		try {
			return window.localStorage;
		} catch (e) {
			return null;
		}
	}

	function resultCacheFor(api) {
		if (api !== cachedApi) {
			cachedApi = api;
			resultCache = snippets ? snippets.createResultCache() : null;
		}
		return resultCache;
	}

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

	// The caps are BYTES (spec/playground.md §1); a UTF-16 `.length`
	// undercounts non-ASCII text.
	function byteLength(text) {
		return new TextEncoder().encode(text).length;
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
			.getElementById("share-hash")
			.addEventListener("click", shareByHash);
		document
			.getElementById("clear-snippets")
			.addEventListener("click", clearCachedSnippets);
		document
			.getElementById("download")
			.addEventListener("click", downloadSource);
		document.getElementById("dropfile").addEventListener("change", (e) => {
			if (e.target.files.length > 0) dropFile(e.target.files[0]);
		});
		window.addEventListener("active-engine-ready", () => {
			// A fresh engine must not reuse results a previous one produced.
			cachedApi = window.ActiveAPI;
			if (snippets) resultCache = snippets.createResultCache();
			if (location.hash.startsWith("#blake3:")) {
				applyHashAddressed();
			} else if (!refused) {
				setStatus("(engine ready: pick an example, then Evaluate)", "");
			}
		});
		window.addEventListener("hashchange", () => {
			if (location.hash.startsWith("#blake3:")) applyHashAddressed();
		});
		// A hash-addressed link needs the engine's hash before anything is
		// shown; do not overwrite it with an example in the meantime.
		if (location.hash.startsWith("#blake3:")) {
			if (!snippets) {
				refuse("Hash links need snippets.js; it did not load.");
			} else if (window.ActiveAPI) {
				// If the module was already instantiated before boot (a warm
				// cache can resolve before this listener exists), apply now.
				applyHashAddressed();
			} else {
				source.value = "# verifying the hash-addressed snippet...";
			}
		} else {
			loadFromHash() || loadExample();
		}
		if (!window.ActiveAPI && !refused) {
			setStatus(
				"Engine not loaded: build active-wasm (W-001) and serve this directory.",
				"",
			);
			diagBox.textContent = "See WIRING.md for the local-dev loop.";
		}
	}

	function loadExample() {
		var name = picker.value || EXAMPLES[0];
		// Only the allowlist may reach the fetch path; a devtools-set
		// picker value must not turn into a same-origin traversal.
		if (EXAMPLES.indexOf(name) < 0) return;
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
		if (location.hash.startsWith("#blake3:")) return false;
		if (location.hash.startsWith("#c=")) {
			loadCompressed();
			return true;
		}
		if (!location.hash.startsWith("#s=")) return false;
		if (location.hash.length > LINK_LIMIT) {
			refuse("Share link refused: it is larger than the 8 KiB snippet cap.");
			return true;
		}
		try {
			var text = decodeURIComponent(location.hash.slice(3));
			if (text.length > SOURCE_LIMIT || byteLength(text) > SOURCE_LIMIT) {
				refuse("Share link refused: the snippet exceeds 8 KiB.");
				return true;
			}
			source.value = text;
			return true;
		} catch (e) {
			refuse("Share link refused: malformed encoding.");
			return true;
		}
	}

	async function loadCompressed() {
		if (location.hash.length > LINK_LIMIT) {
			refuse("Share link refused: it is larger than the 8 KiB snippet cap.");
			return;
		}
		try {
			const bytes = fromB64url(location.hash.slice(3));
			if (!bytes) {
				refuse("Share link refused: malformed compressed payload.");
				return;
			}
			const ds = new DecompressionStream("deflate-raw");
			const writer = ds.writable.getWriter();
			await writer.write(bytes);
			await writer.close();
			// Stream with a hard output cap: a small compressed payload
			// must not expand into an unbounded buffer.
			const reader = ds.readable.getReader();
			const chunks = [];
			let total = 0;
			for (;;) {
				const { done, value } = await reader.read();
				if (done) break;
				total += value.length;
				if (total > SOURCE_LIMIT) {
					try {
						await reader.cancel();
					} catch (e) {
						/* already closed */
					}
					refuse("Share link refused: the decompressed snippet exceeds 8 KiB.");
					return;
				}
				chunks.push(value);
			}
			const out = new Uint8Array(total);
			let at = 0;
			for (const chunk of chunks) {
				out.set(chunk, at);
				at += chunk.length;
			}
			source.value = new TextDecoder("utf-8", { fatal: true }).decode(out);
		} catch (e) {
			refuse("Share link refused: malformed compressed data.");
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
		if (typeof s !== "string" || s.length > LINK_LIMIT) return null;
		if (!/^[A-Za-z0-9_-]+$/.test(s)) return null;
		try {
			const bin = atob(s.replace(/-/g, "+").replace(/_/g, "/"));
			const bytes = new Uint8Array(bin.length);
			for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
			return bytes;
		} catch (e) {
			return null;
		}
	}

	async function shareSource(text) {
		// Prefer compressed links; fall back to raw links where the
		// platform lacks CompressionStream. Take whichever is shorter:
		// compression overhead loses on tiny documents. Past ~8KB of
		// hash, offer download plus file-drop instead of any link.
		if (text.length > SOURCE_LIMIT) {
			diagBox.textContent += "\n[Too large to link: use Download below.]";
			return;
		}
		if (byteLength(text) > SOURCE_LIMIT) {
			diagBox.textContent +=
				"\n[Too large to link (over 8 KiB of UTF-8): use Download below.]";
			return;
		}
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
		if (file.size > SOURCE_LIMIT) {
			refuse("File refused: it exceeds the 8 KiB snippet cap.");
			return;
		}
		const reader = new FileReader();
		reader.onload = () => {
			var text = String(reader.result || "");
			if (text.length > SOURCE_LIMIT || byteLength(text) > SOURCE_LIMIT) {
				refuse("File refused: it exceeds the 8 KiB snippet cap.");
				return;
			}
			source.value = text;
		};
		reader.readAsText(file);
	}

	function renderResult(res) {
		setStatus(res.json, res.ok ? "ok" : "err");
		diagBox.textContent = res.diagnostics;
	}

	function engineHash(text) {
		if (!window.ActiveAPI || typeof window.ActiveAPI.hash !== "function") {
			throw new Error("this engine predates L-P47 (no hash export)");
		}
		return window.ActiveAPI.hash(text);
	}

	function evaluate() {
		if (!window.ActiveAPI) {
			diagBox.textContent = "Engine not loaded.";
			return;
		}
		refused = false;
		try {
			var text = source.value;
			var hashHex = null;
			try {
				hashHex = engineHash(text);
			} catch (e) {
				hashHex = null;
			}
			var results = resultCacheFor(window.ActiveAPI);
			if (hashHex && results && results.has(hashHex)) {
				renderResult(results.get(hashHex));
				return;
			}
			var res = window.ActiveAPI.evaluate(text);
			if (hashHex && results) results.put(hashHex, res);
			renderResult(res);
			// Keep the legacy auto-share, but never replace a hash-addressed
			// link with a legacy one: the hash is the point of that link.
			if (res.ok && !location.hash.startsWith("#blake3:")) {
				shareSource(text);
			}
		} catch (e) {
			setStatus("Host error: " + e.message, "err");
		}
	}

	function refuse(message) {
		refused = true;
		source.value = "";
		setStatus("(refused)", "err");
		diagBox.textContent = message;
	}

	function applyHashAddressed() {
		if (!snippets || !window.ActiveAPI) return;
		var parsed = snippets.parse(location.hash);
		if (!parsed) {
			refuse("Hash link refused: malformed #blake3 fragment.");
			return;
		}
		if (parsed.kind === "self-contained") {
			var text = snippets.decodeSource(parsed.payload);
			if (text === null) {
				refuse(
					"Hash link refused: the payload is not valid UTF-8 or exceeds 8 KiB. Nothing was executed.",
				);
				return;
			}
			if (!snippets.verify(text, parsed.hash, engineHash)) {
				refuse(
					"Hash link refused: the snippet does not match its address. Nothing was executed.",
				);
				return;
			}
			snippets.cachePut(localStore(), parsed.hash, text, engineHash);
			source.value = text;
			evaluate();
			return;
		}
		var cached = snippets.cacheGet(localStore(), parsed.hash, engineHash);
		if (cached === null) {
			refuse(
				"This snippet is not available in this browser. Ask the sender for a self-contained #blake3 link.",
			);
			return;
		}
		source.value = cached;
		evaluate();
	}

	function shareByHash() {
		if (!snippets) {
			diagBox.textContent = "snippets.js is not loaded.";
			return;
		}
		if (!window.ActiveAPI) {
			diagBox.textContent = "Engine not loaded.";
			return;
		}
		try {
			var text = source.value;
			if (text === "") {
				diagBox.textContent = "Nothing to share: the editor is empty.";
				return;
			}
			if (snippets.encodeSource(text) === null) {
				// `encodeSource` fails only on the UTF-8 byte cap: a JS
				// string always encodes. Name the byte cap, since a CJK
				// source can pass the UTF-16 `.length` check and still
				// cross 8 KiB of UTF-8.
				diagBox.textContent =
					"Snippet is over 8 KiB of UTF-8; use Download instead of a hash link.";
				return;
			}
			var hashHex = engineHash(text);
			// A self-contained link carries its own content, so a refused
			// cache write (private mode, quota) must not block sharing.
			// It only means the hash-only form will not reopen here.
			var cached = snippets.cachePut(localStore(), hashHex, text, engineHash);
			var link = snippets.buildSelfContained(text, hashHex);
			if (!link) {
				diagBox.textContent = "Could not build a hash link.";
				return;
			}
			history.replaceState(null, "", link);
			shareLink.textContent = location.href;
			diagBox.textContent = cached
				? "Self-verifying hash link (verifies before it runs): " + location.href
				: "Self-verifying hash link (verifies before it runs). The local cache refused it; the link still works, but a hash-only reopening will not resolve in this browser: " +
					location.href;
		} catch (e) {
			diagBox.textContent = "Hash link failed: " + e.message;
		}
	}

	function clearCachedSnippets() {
		if (!snippets) return;
		snippets.cacheClear(localStore());
		shareLink.textContent = "";
		diagBox.textContent =
			"Cleared this browser's cached snippets (links and files are untouched).";
	}

	document.addEventListener("DOMContentLoaded", boot);
})();

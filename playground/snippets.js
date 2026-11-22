/* Hash-addressed playground snippets (L-P47): fragment parsing/building,
 * verification, and the bounded local cache. UMD with no DOM access so the
 * Node test can exercise exactly the code the page runs.
 *
 * A self-contained link is `#blake3:<64 hex>:<base64url source>`; the hash
 * verifies the bytes before anything is shown or evaluated. A hash-only
 * link (`#blake3:<64 hex>`) resolves from the bounded local cache and is
 * re-verified on every read. Nothing here fetches from a network: the
 * content-addressed registry (L-P31) is a later, separate design.
 */
((root, factory) => {
	if (typeof module === "object" && module.exports) module.exports = factory();
	else root.ActiveSnippets = factory();
})(typeof self !== "undefined" ? self : this, () => {
	var PREFIX = "#blake3:";
	var HEX = /^[0-9a-f]{64}$/;
	var MAX_SOURCE_BYTES = 8192;
	var CACHE_LIMIT = 64;
	var CACHE_BYTES_LIMIT = 1024 * 1024;
	var CACHE_PREFIX = "active.snippet.";
	var RESULT_LIMIT = 64;
	// A session cache is a convenience; a single huge value must not pin
	// the tab. Results above this combined JSON+diagnostics length are not
	// cached (re-evaluated on demand instead).
	var RESULT_BYTES_LIMIT = 262144;
	// Base64url for 8192 bytes, plus slack for padding-free encodings.
	var MAX_PAYLOAD = Math.ceil((MAX_SOURCE_BYTES * 4) / 3) + 4;

	function isHash(value) {
		return typeof value === "string" && HEX.test(value);
	}

	function utf8(text) {
		return new TextEncoder().encode(text);
	}

	// Storage is a convenience, never a requirement: private mode or a
	// quota error must not prevent a self-contained link from running.
	function storageGet(storage, key) {
		try {
			return storage.getItem(key);
		} catch (e) {
			return null;
		}
	}

	function storageSet(storage, key, value) {
		try {
			storage.setItem(key, value);
			return true;
		} catch (e) {
			return false;
		}
	}

	function storageRemove(storage, key) {
		try {
			storage.removeItem(key);
		} catch (e) {
			/* best effort */
		}
	}

	function storageKey(storage, index) {
		try {
			return storage.key(index);
		} catch (e) {
			return null;
		}
	}

	function storageLength(storage) {
		try {
			return storage.length;
		} catch (e) {
			return 0;
		}
	}

	function b64urlEncode(bytes) {
		if (typeof Buffer !== "undefined") {
			return Buffer.from(bytes).toString("base64url");
		}
		var bin = "";
		var CHUNK = 8192;
		for (var i = 0; i < bytes.length; i += CHUNK) {
			bin += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
		}
		return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
	}

	function b64urlDecode(text) {
		if (typeof text !== "string" || text.length === 0) return null;
		// Bound before decoding: a megabyte payload must not allocate.
		if (text.length > MAX_PAYLOAD) return null;
		// Canonical alphabet only; Node's Buffer.from is otherwise lax
		// (`+`, `/`, `=`, whitespace) and would accept a non-canonical link.
		if (!/^[A-Za-z0-9_-]+$/.test(text)) return null;
		try {
			if (typeof Buffer !== "undefined") {
				var buf = Buffer.from(text, "base64url");
				if (buf.length === 0) return null;
				return new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
			}
			var bin = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
			var bytes = new Uint8Array(bin.length);
			for (var j = 0; j < bin.length; j++) bytes[j] = bin.charCodeAt(j);
			return bytes;
		} catch (e) {
			return null;
		}
	}

	function encodeSource(source) {
		if (typeof source !== "string") return null;
		var bytes = utf8(source);
		if (bytes.length > MAX_SOURCE_BYTES) return null;
		return b64urlEncode(bytes);
	}

	function decodeSource(payload) {
		var bytes = b64urlDecode(payload);
		if (!bytes) return null;
		if (bytes.length > MAX_SOURCE_BYTES) return null;
		try {
			return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
		} catch (e) {
			return null;
		}
	}

	function buildSelfContained(source, hashHex) {
		if (!isHash(hashHex)) return null;
		// An empty source encodes to an empty payload, which `parse`
		// refuses as malformed; never build a link that cannot round-trip.
		if (source === "") return null;
		var payload = encodeSource(source);
		if (payload === null) return null;
		return PREFIX + hashHex + ":" + payload;
	}

	/** `{kind:"self-contained", hash, payload}` | `{kind:"hash", hash}` | null */
	function parse(fragment) {
		if (typeof fragment !== "string" || fragment.indexOf(PREFIX) !== 0) {
			return null;
		}
		var body = fragment.slice(PREFIX.length);
		var colon = body.indexOf(":");
		if (colon < 0) {
			return isHash(body) ? { kind: "hash", hash: body } : null;
		}
		var hash = body.slice(0, colon);
		var payload = body.slice(colon + 1);
		if (!isHash(hash) || payload.length === 0) return null;
		return { kind: "self-contained", hash: hash, payload: payload };
	}

	function sameHex(a, b) {
		if (
			typeof a !== "string" ||
			typeof b !== "string" ||
			a.length !== b.length
		) {
			return false;
		}
		var diff = 0;
		for (var i = 0; i < a.length; i++)
			diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
		return diff === 0;
	}

	function verify(source, hashHex, hashSource) {
		if (!isHash(hashHex) || typeof hashSource !== "function") return false;
		try {
			return sameHex(hashSource(source), hashHex);
		} catch (e) {
			return false;
		}
	}

	function cacheKey(hashHex) {
		return CACHE_PREFIX + hashHex;
	}

	function cachePut(storage, hashHex, source, hashSource, at) {
		if (!storage || !isHash(hashHex) || typeof source !== "string")
			return false;
		if (utf8(source).length > MAX_SOURCE_BYTES) return false;
		if (!verify(source, hashHex, hashSource)) return false;
		var stamp = String(at === undefined ? Date.now() : at);
		var stampKey = cacheKey(hashHex) + ".at";
		if (!storageSet(storage, cacheKey(hashHex), source)) return false;
		// The data key and its timestamp are one entry: if the second
		// write fails (quota), remove the first. A dataless timestamp
		// would otherwise read as an entry at 0 and disturb LRU order.
		if (!storageSet(storage, stampKey, stamp)) {
			storageRemove(storage, cacheKey(hashHex));
			return false;
		}
		prune(storage);
		return true;
	}

	function cacheGet(storage, hashHex, hashSource) {
		if (!storage || !isHash(hashHex)) return null;
		var source = storageGet(storage, cacheKey(hashHex));
		if (source === null) return null;
		// A hand-planted entry must obey the same size cap as a link.
		if (utf8(source).length > MAX_SOURCE_BYTES) {
			storageRemove(storage, cacheKey(hashHex));
			storageRemove(storage, cacheKey(hashHex) + ".at");
			return null;
		}
		if (!verify(source, hashHex, hashSource)) {
			// A tampered or corrupt entry is refused and removed; it must
			// never be shown or evaluated.
			storageRemove(storage, cacheKey(hashHex));
			storageRemove(storage, cacheKey(hashHex) + ".at");
			return null;
		}
		return source;
	}

	function cacheEntries(storage) {
		var entries = [];
		if (!storage) return entries;
		var count = storageLength(storage);
		for (var i = 0; i < count; i++) {
			var key = storageKey(storage, i);
			if (typeof key !== "string" || key.indexOf(CACHE_PREFIX) !== 0) continue;
			if (key.slice(-3) === ".at") continue;
			var source = storageGet(storage, key);
			if (source === null) continue;
			var stamp = Number(storageGet(storage, key + ".at"));
			entries.push({
				key: key,
				hash: key.slice(CACHE_PREFIX.length),
				source: source,
				at: Number.isFinite(stamp) ? stamp : 0,
			});
		}
		return entries;
	}

	function prune(storage) {
		var entries = cacheEntries(storage);
		var total = 0;
		for (var i = 0; i < entries.length; i++)
			total += utf8(entries[i].source).length;
		if (entries.length <= CACHE_LIMIT && total <= CACHE_BYTES_LIMIT) return;
		entries.sort((a, b) => a.at - b.at || (a.key < b.key ? -1 : 1));
		var index = 0;
		while (
			index < entries.length &&
			(entries.length - index > CACHE_LIMIT || total > CACHE_BYTES_LIMIT)
		) {
			var entry = entries[index++];
			total -= utf8(entry.source).length;
			storageRemove(storage, entry.key);
			storageRemove(storage, entry.key + ".at");
		}
	}

	function cacheClear(storage) {
		if (!storage) return;
		var doomed = [];
		var count = storageLength(storage);
		for (var i = 0; i < count; i++) {
			var key = storageKey(storage, i);
			if (typeof key === "string" && key.indexOf(CACHE_PREFIX) === 0)
				doomed.push(key);
		}
		doomed.forEach((key) => {
			storageRemove(storage, key);
		});
	}

	function cacheCount(storage) {
		return cacheEntries(storage).length;
	}

	/** In-memory result cache for the page session: hash -> evaluate result. */
	function createResultCache() {
		var map = new Map();
		function touch(hashHex) {
			if (!map.has(hashHex)) return undefined;
			var value = map.get(hashHex);
			map.delete(hashHex);
			map.set(hashHex, value);
			return value;
		}
		return {
			has: (hashHex) => {
				if (!map.has(hashHex)) return false;
				touch(hashHex);
				return true;
			},
			get: (hashHex) => touch(hashHex),
			put: (hashHex, result) => {
				if (!isHash(hashHex) || !result || typeof result !== "object") return;
				var text =
					String((result && result.json) || "") +
					String((result && result.diagnostics) || "");
				if (utf8(text).length > RESULT_BYTES_LIMIT) return;
				// A cached result is shared by every hit; a caller that
				// mutates the record must not poison later evaluations.
				// The shim already deep-freezes engine results; this
				// covers a hand-built or test result as well.
				if (!Object.isFrozen(result)) Object.freeze(result);
				if (map.has(hashHex)) {
					map.delete(hashHex);
				} else if (map.size >= RESULT_LIMIT) {
					// True LRU: `has`/`get` refresh order, so a hot entry is
					// not evicted as "oldest" merely because it was inserted
					// first.
					var oldest = map.keys().next();
					if (!oldest.done) map.delete(oldest.value);
				}
				map.set(hashHex, result);
			},
			size: () => map.size,
		};
	}

	return {
		PREFIX: PREFIX,
		MAX_SOURCE_BYTES: MAX_SOURCE_BYTES,
		MAX_PAYLOAD: MAX_PAYLOAD,
		CACHE_LIMIT: CACHE_LIMIT,
		CACHE_BYTES_LIMIT: CACHE_BYTES_LIMIT,
		CACHE_PREFIX: CACHE_PREFIX,
		RESULT_LIMIT: RESULT_LIMIT,
		RESULT_BYTES_LIMIT: RESULT_BYTES_LIMIT,
		isHash: isHash,
		encodeSource: encodeSource,
		decodeSource: decodeSource,
		buildSelfContained: buildSelfContained,
		parse: parse,
		verify: verify,
		cacheKey: cacheKey,
		cachePut: cachePut,
		cacheGet: cacheGet,
		cacheClear: cacheClear,
		cacheCount: cacheCount,
		createResultCache: createResultCache,
	};
});

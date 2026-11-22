#!/usr/bin/env node
/* L-P47 gate: the hash-addressed snippet module against the real engine.
 *
 * Run: node playground/test-snippets.js <path-to-active_wasm.wasm>
 *
 * Exercises exactly the code the page runs (playground/snippets.js) with
 * the real BLAKE3 export from the release-shaped wasm. Node counters only:
 * no wall-clock assertions. The negative control is manual and documented
 * in the design record: skip the verification call in cacheGet and watch
 * the tampered-entry check fail; restore.
 */

const assert = require("assert");
const fs = require("fs");
const path = require("path");
const shim = require(
	path.join(__dirname, "..", "active-wasm", "js", "active-shim.js"),
);
const snippets = require(path.join(__dirname, "snippets.js"));

let checks = 0;
function ok(value, message) {
	assert.ok(value, message);
	checks += 1;
}

function fakeStorage() {
	const map = new Map();
	return {
		get length() {
			return map.size;
		},
		key(index) {
			const keys = Array.from(map.keys());
			return index < keys.length ? keys[index] : null;
		},
		getItem(key) {
			return map.has(key) ? map.get(key) : null;
		},
		setItem(key, value) {
			map.set(String(key), String(value));
		},
		removeItem(key) {
			map.delete(key);
		},
	};
}

async function main() {
	const wasmPath = process.argv[2];
	if (!wasmPath) {
		throw new Error(
			"usage: node playground/test-snippets.js <active_wasm.wasm>",
		);
	}
	const bytes = fs.readFileSync(wasmPath);
	const buffer = bytes.buffer.slice(
		bytes.byteOffset,
		bytes.byteOffset + bytes.byteLength,
	);
	const api = await shim.load(buffer);
	const hash = (text) => api.hash(text);

	// An HTTP failure is not a module; the loader must reject it so the
	// page's fallback source can run instead of parsing a 404 page.
	let httpRefused = false;
	try {
		await shim.load({ ok: false, status: 404 });
	} catch (e) {
		httpRefused = /404/.test(String(e && e.message));
	}
	ok(httpRefused, "an HTTP error refuses the load");

	// 1. The engine hash is the shape the URL carries.
	const source = 'server = { port = 8080, host = "localhost" }\n';
	const digest = hash(source);
	ok(
		/^[0-9a-f]{64}$/.test(digest),
		"engine hash is 64 lowercase hex characters",
	);
	ok(hash(source) === digest, "hash is deterministic");
	ok(hash(source + " ") !== digest, "hash changes with the bytes");
	// A pinned vector: a hasher or domain change breaks every existing
	// link (they refuse), so the gate must fail before such a change
	// ships. The empty digest is BLAKE3's published constant.
	ok(
		hash("port = 8080\n") ===
			"19fefa6f1e98c3e2f9b3b093a5bfe8eecae555579f78bc80bd1cc531515c7970",
		"BLAKE3 source vector is pinned",
	);
	ok(
		hash("") ===
			"af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
		"BLAKE3 empty vector is pinned",
	);

	// 2. Self-contained round trip.
	const link = snippets.buildSelfContained(source, digest);
	ok(link.startsWith("#blake3:" + digest + ":"), "self-contained prefix");
	const parsed = snippets.parse(link);
	ok(parsed && parsed.kind === "self-contained", "parse kind");
	ok(parsed.hash === digest, "parse hash");
	const decoded = snippets.decodeSource(parsed.payload);
	ok(decoded === source, "payload round-trips exactly");
	ok(snippets.verify(decoded, parsed.hash, hash), "matching bytes verify");

	// An empty source has no self-contained form: the payload would be
	// empty and `parse` refuses it as malformed, so never build one.
	const emptyHash = hash("");
	ok(
		snippets.buildSelfContained("", emptyHash) === null,
		"an empty source builds no link",
	);
	ok(
		snippets.parse("#blake3:" + emptyHash + ":") === null,
		"an empty payload does not parse",
	);

	// Short sources encode to unpadded base64url lengths 2/3/4/6/7; the
	// browser's `atob` needs the padding restored.
	for (const short of ["a", "ab", "abc", "abcd", "abcde"]) {
		const shortHash = hash(short);
		const shortLink = snippets.buildSelfContained(short, shortHash);
		const shortParsed = snippets.parse(shortLink);
		ok(
			shortParsed !== null &&
				snippets.decodeSource(shortParsed.payload) === short,
			"short source round-trips: " + short.length + " bytes",
		);
	}

	// 3. A mismatch is a refusal, never a pass.
	ok(
		!snippets.verify(decoded + "x", parsed.hash, hash),
		"wrong bytes do not verify",
	);
	const other = hash("port = 9090\n");
	const forged = link.replace(digest, other);
	const forgedParsed = snippets.parse(forged);
	ok(
		forgedParsed &&
			!snippets.verify(
				snippets.decodeSource(forgedParsed.payload),
				forgedParsed.hash,
				hash,
			),
		"a forged hash refuses",
	);

	// 4. Hash-only form parses; malformed forms do not.
	const hashOnly = snippets.parse("#blake3:" + digest);
	ok(
		hashOnly && hashOnly.kind === "hash" && hashOnly.hash === digest,
		"hash-only parses",
	);
	for (const bad of [
		"#blake3:",
		"#blake3:XYZ",
		"#blake3:" + digest.toUpperCase(),
		"#blake3:" + digest + ":",
		"#sha256:" + digest,
		"#blake3:" + digest.slice(1) + ":AA",
	]) {
		ok(snippets.parse(bad) === null, "malformed fragment refuses: " + bad);
	}

	// 5. The 8 KiB cap is enforced on encode and decode.
	const big = "x".repeat(snippets.MAX_SOURCE_BYTES + 1);
	ok(snippets.encodeSource(big) === null, "oversized source does not encode");
	ok(
		snippets.buildSelfContained(big, hash(big)) === null,
		"oversized source builds no link",
	);
	const just = "y".repeat(snippets.MAX_SOURCE_BYTES);
	ok(snippets.encodeSource(just) !== null, "a source at the cap encodes");
	// L-P47 round 3: the cap is UTF-8 BYTES, so 3000 CJK characters
	// (9000 bytes, 3000 UTF-16 units) must be refused even though its
	// `.length` is far below the cap.
	const cjk = "\u4e2d".repeat(3000);
	ok(
		cjk.length < snippets.MAX_SOURCE_BYTES,
		"CJK source passes the unit check",
	);
	ok(
		new TextEncoder().encode(cjk).length > snippets.MAX_SOURCE_BYTES,
		"CJK source exceeds the byte cap",
	);
	ok(
		snippets.encodeSource(cjk) === null,
		"a CJK source over the UTF-8 byte cap does not encode",
	);

	// 6. Cache: verified hit, tampered entry refused and removed.
	const storage = fakeStorage();
	ok(
		snippets.cachePut(storage, digest, source, hash, 1),
		"cache put verifies and stores",
	);
	ok(
		snippets.cacheGet(storage, digest, hash) === source,
		"cache hit returns the source",
	);
	storage.setItem(snippets.cacheKey(digest), source + "// tampered\n");
	ok(
		snippets.cacheGet(storage, digest, hash) === null,
		"tampered cache entry refuses",
	);
	ok(
		storage.getItem(snippets.cacheKey(digest)) === null,
		"tampered entry is removed",
	);

	// The data key and its timestamp are one entry: a failed timestamp
	// write removes the data key instead of leaving a dataless entry.
	const failing = fakeStorage();
	const realSet = failing.setItem;
	let writes = 0;
	failing.setItem = (key, value) => {
		writes += 1;
		if (writes === 2) throw new Error("quota");
		realSet(key, value);
	};
	ok(
		!snippets.cachePut(failing, digest, source, hash, 1),
		"a failed timestamp write refuses the put",
	);
	ok(
		failing.getItem(snippets.cacheKey(digest)) === null,
		"a failed timestamp leaves no data key",
	);

	// 7. Cache bound: the oldest entries evict first.
	const bounded = fakeStorage();
	for (let i = 0; i < snippets.CACHE_LIMIT + 8; i += 1) {
		const text = "v = " + i + "\n";
		ok(
			snippets.cachePut(bounded, hash(text), text, hash, 100 + i),
			"cache insert " + i,
		);
	}
	ok(
		snippets.cacheCount(bounded) <= snippets.CACHE_LIMIT,
		"cache count stays within the cap",
	);
	const first = "v = 0\n";
	ok(
		snippets.cacheGet(bounded, hash(first), hash) === null,
		"the oldest entry was evicted",
	);
	const last = "v = " + (snippets.CACHE_LIMIT + 7) + "\n";
	ok(
		snippets.cacheGet(bounded, hash(last), hash) === last,
		"the newest entry survives",
	);
	snippets.cacheClear(bounded);
	ok(snippets.cacheCount(bounded) === 0, "clear removes every cached snippet");

	// A devtools-planted key that is not hash-shaped must not inflate the
	// count or the byte total and evict a legitimate entry.
	const junk = fakeStorage();
	junk.setItem("active.snippet.junk", "x".repeat(4096));
	ok(snippets.cacheCount(junk) === 0, "a non-hash cache key is ignored");
	ok(
		snippets.cachePut(junk, digest, source, hash, 1),
		"a real entry still caches beside junk",
	);
	ok(snippets.cacheCount(junk) === 1, "only the real entry counts");
	ok(
		junk.getItem("active.snippet.junk") !== null,
		"the junk key is left alone, not counted",
	);

	// 8. Result cache: the hash is the key, so the same bytes hit and the
	// bound evicts the oldest entry.
	const results = snippets.createResultCache();
	ok(!results.has(digest), "result cache starts empty");
	const cachedResult = { ok: true, json: "{}" };
	results.put(digest, cachedResult);
	ok(results.has(digest), "result cache records the hash");
	ok(results.get(digest).ok === true, "result cache returns the stored result");
	// A hit is shared by every caller; freezing the record keeps one
	// caller from poisoning the cache for the next.
	ok(Object.isFrozen(results.get(digest)), "a cached result is frozen");
	try {
		results.get(digest).ok = false;
	} catch (e) {
		/* frozen assignment throws in strict mode only */
	}
	ok(
		results.get(digest).ok === true,
		"mutating a cached result does not poison the cache",
	);
	// The stored record is a detached copy, frozen at every level: a
	// caller cannot poison a hit through a nested `items` alias either.
	const itemsSource = "items\n";
	const itemsKey = hash(itemsSource);
	const original = {
		ok: true,
		json: "{}",
		diagnostics: "",
		items: [{ code: "T001" }],
	};
	results.put(itemsKey, original);
	const hit = results.get(itemsKey);
	ok(
		Array.isArray(hit.items) && Object.isFrozen(hit.items),
		"items are frozen",
	);
	try {
		hit.items.push({ code: "E999" });
	} catch (e) {
		/* frozen array append throws */
	}
	original.items.push({ code: "E999" });
	ok(
		results.get(itemsKey).items.length === 1,
		"nested items and the caller's object cannot poison the cache",
	);
	// The cap counts the whole record: huge `items` with a tiny `json`
	// must not be cached.
	const itemCap = snippets.createResultCache();
	const hugeItemsKey = hash("huge-items\n");
	itemCap.put(hugeItemsKey, {
		ok: true,
		json: "{}",
		diagnostics: "",
		items: [{ message: "x".repeat(snippets.RESULT_BYTES_LIMIT) }],
	});
	ok(!itemCap.has(hugeItemsKey), "a huge items array is not cached");
	ok(results.size() === 2, "result cache size (digest + items)");
	for (let i = 0; i < snippets.RESULT_LIMIT + 5; i += 1) {
		results.put(hash("result " + i + "\n"), { ok: true, json: "{}" });
	}
	ok(
		results.size() <= snippets.RESULT_LIMIT,
		"result cache stays within its bound",
	);
	ok(!results.has(digest), "the oldest result was evicted");

	// True LRU: a touched entry survives an overflow, the untouched one goes.
	const lru = snippets.createResultCache();
	const lruA = hash("a\n");
	const lruB = hash("b\n");
	lru.put(lruA, { ok: true, json: "{}" });
	lru.put(lruB, { ok: true, json: "{}" });
	for (let i = 0; i < snippets.RESULT_LIMIT - 2; i += 1) {
		lru.put(hash("k" + i + "\n"), { ok: true, json: "{}" });
	}
	ok(lru.size() === snippets.RESULT_LIMIT, "LRU filled to the cap");
	lru.has(lruA);
	lru.put(hash("overflow\n"), { ok: true, json: "{}" });
	ok(lru.has(lruA), "a touched entry survives the overflow");
	ok(!lru.has(lruB), "the untouched oldest entry was evicted");

	// A single huge result is not cached (re-evaluated on demand).
	const huge = snippets.createResultCache();
	const hugeKey = hash("huge\n");
	huge.put(hugeKey, {
		ok: true,
		json: "x".repeat(snippets.RESULT_BYTES_LIMIT + 1),
		diagnostics: "",
	});
	ok(!huge.has(hugeKey), "an oversized result is not cached");

	// 9. Storage failure never blocks a self-contained link: the cache is
	// best-effort, so put/get return honestly instead of throwing.
	const throwing = {
		get length() {
			throw new Error("denied");
		},
		key() {
			throw new Error("denied");
		},
		getItem() {
			throw new Error("denied");
		},
		setItem() {
			throw new Error("denied");
		},
		removeItem() {
			throw new Error("denied");
		},
	};
	ok(
		!snippets.cachePut(throwing, digest, source, hash, 1),
		"throwing storage put is false",
	);
	ok(
		snippets.cacheGet(throwing, digest, hash) === null,
		"throwing storage get is a miss",
	);
	ok(snippets.cacheCount(throwing) === 0, "throwing storage counts nothing");
	snippets.cacheClear(throwing);

	// 10. A hand-planted oversize entry is refused and removed, even with
	// a valid hash (the link cap applies to cache reads too).
	const oversize = "z".repeat(snippets.MAX_SOURCE_BYTES + 1);
	const oversizeHash = hash(oversize);
	const planted = fakeStorage();
	planted.setItem(snippets.cacheKey(oversizeHash), oversize);
	ok(
		snippets.cacheGet(planted, oversizeHash, hash) === null,
		"oversize cache entry refuses",
	);
	ok(
		planted.getItem(snippets.cacheKey(oversizeHash)) === null,
		"oversize cache entry is removed",
	);

	// 11. Payload guards: an oversized or non-canonical payload is refused
	// before decoding.
	ok(
		snippets.decodeSource("A".repeat(1024 * 1024)) === null,
		"1 MiB payload refuses",
	);
	ok(snippets.decodeSource("ab+cd") === null, "plus is not base64url");
	ok(snippets.decodeSource("ab/cd") === null, "slash is not base64url");
	ok(snippets.decodeSource("abcd==") === null, "padding is not accepted");

	console.log("SNIPPETS-OK (" + checks + " checks)");
}

main().catch((error) => {
	console.error("SNIPPETS-FAIL:", error && error.stack ? error.stack : error);
	process.exit(1);
});

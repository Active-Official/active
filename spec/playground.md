# Playground snippets (L-P47)

The playground is a static page (HTML + `playground.js` + the zero-import
`active_wasm.wasm`). This section is the normative contract for
hash-addressed snippets. It is page behaviour, not evaluation semantics:
the engine still comes from the release wasm (LANG-G-023) and every
snippet runs through the same `ActiveAPI.evaluate` the CLI-shape engine
provides.

## 1. Fragment scheme

| Form | Meaning |
|---|---|
| `#blake3:<64 hex>:<base64url source>` | Self-contained: the fragment carries the exact UTF-8 source; the hash verifies it. This is the shareable form the page produces. |
| `#blake3:<64 hex>` | Hash-only: resolve from the browser's bounded local cache; a miss is a refusal. |
| `#c=...` / `#s=...` | Legacy G-019 share forms; unchanged. |

The hash is the BLAKE3 digest (lowercase hex) of the source bytes, the
same hasher the project uses for content identity. The base64url payload
is the raw UTF-8 source (no compression dependency). The source is capped
at 8192 bytes, matching the G-019 link cap. A fragment is never sent to
the server (browsers do not transmit fragments).

## 2. Verification before execution

The page MUST, in this order:

1. Parse the fragment; an unknown or malformed `#blake3:` shape refuses.
2. Decode the payload with the byte cap; a decode failure or invalid
   UTF-8 refuses.
3. Hash the decoded source with the engine and compare in constant time.
4. On mismatch, refuse: clear the editor, show the refusal, execute
   nothing, write nothing to the cache. A mismatch is never a warning.
5. On match, place the source and evaluate. A hash-only cache hit is
   re-hashed in exactly the same way before use.

The page never executes a snippet without a successful hash comparison,
and it has no page-side evaluation logic (the WIRING contract).

## 3. Caches

- **Source cache:** `localStorage`, key `active.snippet.<hash>`. Bounded
  to 64 entries and 1 MiB total, oldest-first eviction, cleared by the
  page's "Clear cached snippets" action. Every read is re-verified; a
  tampered or corrupt entry is deleted and treated as a miss.
- **Result cache:** an in-memory `Map` for the page session, keyed by the
  hash. Re-opening the same hash or re-evaluating the same bytes is a hit.
  It is not persisted, so a new engine build never serves a stale result
  across sessions.
- The hash is the cache key in both caches: identical bytes map to one
  entry, and different bytes cannot occupy an existing hash.

## 4. Retention and abuse

- The page has **no server-side storage and no upload endpoint**. A
  snippet lives in the URL fragment or in the visitor's own browser
  cache; there is no pastebin to moderate, and the site retains nothing.
- The local cache is bounded, user-clearable, and re-verified on read, so
  it cannot become an unbounded store or execute tampered bytes.
- The 8 KiB cap is enforced on encode, decode, and cache writes. Above
  the cap the page offers download/file-drop instead (G-019).
- A hash-only link with no local entry refuses visibly; the page never
  fetches a hash from a network. A content-addressed registry is a
  separate, later design (L-P31) with its own retention rules.
- Hostile content can only affect the visitor who opens the link: the
  engine's budgets apply, secrets stay structural, and the page performs
  no `eval`.

## 5. Host

`playground.active.dev` is a deployment label used in the proposal; the
scheme is host-independent and works on any static server serving the
page. This specification does not claim the domain is live.

## 6. Engine prerequisite

The page requires the release wasm next to it (`playground/active_wasm.wasm`,
gitignored, staged by `playground/fetch-wasm.py` locally or by the
release wasm job, which also publishes `active-playground.tar.gz`). A
missing engine shows the honest empty state and never fakes a result.

## 7. Out of scope

Server-side snippet storage, hash-only cross-user retrieval, and any
registry interaction are out of scope until L-P31 is designed and
approved. The local cache and self-contained links are the whole feature.

# `active serve`: stdio document transport (L-T05)

`active serve` is the stdio member of the transport set (DEC-001): a
long-lived process that reads document requests from stdin and writes
evaluated results to stdout. It is framing, not a text protocol: Active
documents contain newlines, so newline-delimited framing cannot carry
them. This is the same family as LSP's transport, with a hard cap the LSP
path does not have.

## 1. Framing

Every frame is a `u32` little-endian payload length followed by exactly
that many bytes:

```
+----------------+----------------------------+
| length (u32 LE)| payload (length bytes)     |
+----------------+----------------------------+
```

- The length is checked against the frame cap **before any payload
  allocation**. An over-cap length is answered with one error frame and
  the server stops reading (a stream cannot be resynchronized past an
  unread payload); it never allocates the claimed size and never blocks
  waiting for bytes it will not read.
- Default cap: 16 MiB (`--max-frame`). Rationale, not a round number: the
  parse input cap is 4 MiB (`spec/budgets.md`), the largest shipped
  document is about 250 KB, and 16 MiB leaves four times the parse cap
  of headroom for generated documents while bounding one frame's
  allocation at 16 MiB. A smaller cap breaks legitimate large documents;
  a larger one weakens the pre-allocation guarantee. The accepted range
  is 256 bytes to `u32::MAX`: below that, the server's own error frames
  (which carry a diagnostic message) could not be framed.
- A response that would exceed the cap is replaced with an error frame
  (`E029`); the request id is echoed. Error frames are small by
  construction (a code and a short message) and fit the minimum cap; a
  too-large echoed id is dropped from the error frame rather than making
  the refusal unframable. The connection continues.

## 2. Requests

One JSON object per frame:

```json
{ "id": 1, "source": "port = 8080\n", "name": "svc", "env": { "K": "V" } }
```

- `id` (any JSON scalar) is echoed on the response; it is how a client
  matches frames. An array or object id is E017.
- `source` (string, required) is the document text.
- `name` (string, optional) labels diagnostics and the envelope file
  field; default `serve`. A non-string name is E017.
- `env` (object of strings, optional) is the **only** environment the
  document can read. The server never consults the process environment.
  A non-object `env`, or a non-string value, is E017 (never silently
  dropped).

Malformed JSON, a missing `source`, or any field of the wrong type is
answered with an error frame `{ "id": <id or null>, "error": { "code":
"E017", "message": ... } }` and the stream continues (the frame
boundaries are intact).

## 3. Responses

One frame per request:

```json
{ "id": 1, "envelope": { "version": "2.0.0", "capability": "agent",
  "tainted": false, "payload": { ... }, "receipt": { ... },
  "result": "sha256:..." } }
```

The envelope is the shared L-T06 tool-result envelope
(`spec/tool-result-envelope.md`) with the same fields, redaction, and
hash binding as `active eval --envelope`: `payload` is the redacted
evaluated tree on success and `null` on failure; `receipt` carries
`outcome`, `failure_class` (`parse`, `budget`, `validation`, `other`),
`capability`, `mode`, the resolved budget, the schema version, and the
diagnostic codes; `result` is the bound content hash and is present only
on a clean outcome. A document hashed through `serve` hashes identically
through direct evaluation.

Redaction uses the same machinery as every export: evaluation collects
its runtime secret seeds (`evaluate_with_seeds`), the profile is built
with those seeds and completed, and a profile that hits its analysis
bounds refuses the envelope instead of emitting a possibly wrong tree
(`failure_class: budget`, `E029`). A material profile or schema-version
selector is never written in cleartext: the envelope is refused with
`E027` (the evaluator's material-selector code) and no selector bound. A
budget trip's receipt keeps the progressive diagnostics (W013 with the
X-code), exactly as eval reports them.

A failure that happens before parsing (an over-cap source, a parse error)
still records the resolved budget in its receipt; `mode` and the schema
version are absent because no document was parsed to supply them. The
server applies the process-wide parse limits (`--max-input-bytes`,
`--parse-wall-ms`) like every other parse path. A dangling selector (a
named profile that does not exist) is an evaluation failure (W002), as
everywhere else; an explicit `--profile` that names nothing never falls
back to the document's `active_profile`, and no selector is bound into
the receipt.

## 4. Budgets before parse

The server refuses a source longer than the parse input cap before
tokenizing a byte: the response is a failed envelope with `failure_class:
parse` and `E029`. This is the same limit the parser enforces; the
server's check exists so the work is refused before it starts. Budgets
are never silently raised by a request; the server's budget (flags, else
the shipped default) is what every request runs under, and it is recorded
in the receipt.

## 5. Hermeticity

- No include reads: an `include` statement is a parse diagnostic (E015),
  never a filesystem read. A document transported over this wire must be
  self-contained (the agent interchange profile's rule).
- No ambient environment: `env.*` resolves only against the request's
  `env` object.
- No sockets: stdio only; the network transport is RPC (DEC-001).

## 6. State, shutdown, and failures

The server holds no evaluation state between requests (no cache, no open
documents); every request is evaluated from its own bytes under the same
engine settings. EOF on stdin ends the server cleanly; an over-cap frame
ends it after one error frame. There is no in-band shutdown frame.

- `--max-frame` must be between 256 bytes and `u32::MAX`; anything else is
  a usage error at startup (a cap outside the length field cannot be
  framed, and a cap below the error-frame size could not carry a refusal).
- A partial frame header (1–3 bytes then EOF) is a torn stream, not a
  clean shutdown: the server exits non-zero with no frame.
- A read or write error on stdin/stdout ends the server with a non-zero
  exit and no response frame: the stream's boundaries are no longer
  trustworthy, so the client sees EOF rather than a frame it cannot
  attribute. A payload allocation that cannot be reserved ends the server
  the same way (a clean `try_reserve` failure is never an abort).
- There is no per-frame read deadline: a client that stops mid-frame
  keeps the server waiting (a stdio client owns its pipe).

## 7. Non-goals

`why`/`impact`/`graph` requests, streaming, and server-side caches are
follow-ups; the first version is evaluation + diagnostics + receipt.

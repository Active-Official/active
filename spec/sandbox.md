# Hermetic evaluation sandbox (L-P28)

**Status:** normative. A sandboxed evaluation is a pure function of an
explicit `InputClosure`; it reads no ambient environment, filesystem,
network, clock, or host fallback. The refusal code is **E048**.

## 1. The closure

```
InputClosure {
  document:      text + diagnostic source name (metadata, not a path)
  includes:      root-relative identity -> exact UTF-8 bytes + SHA-256
  env:           folded name -> Some(value) | None (declared absent)
  profile_override: host selection | None
}
```

- Include keys are root-relative with `/` separators. Absolute keys, a
  backslash, a drive prefix, a `..` segment, an empty key, a control
  character, or a raw spelling longer than 1024 bytes are construction
  errors (the cap is on the raw spelling; a shorter normalized form does
  not rescue it). The synthetic root filename `.active-root` is not an
  include: a path resolving to it is refused, so it can never alias the
  document identity.
- The digest is verified at construction and again by the loader before
  bytes are parsed; a mismatch is E048. Include keys are echoed in
  diagnostics as `include:<key>`; the document itself uses
  `source_name`. An inline `sha256:` pin, when
  present, remains E041.
- Env names fold to uppercase. A duplicate after folding is a
  construction error; the explicit closure never silently picks one.
  `None` is an input (the document's `?:`/null path applies); a name
  absent from the map is outside the closure and is E048 on read.
- An include spelling that is absolute, drive/UNC-qualified, contains
  `..`, or otherwise escapes is refused by the parser's containment
  layer as E015/E016 before the closure loader runs; E048 is the
  closure-loader and guard class (missing identity, digest mismatch,
  undeclared env/profile/fallback). Hosts that branch on "outside the
  closure" handle both classes.
- The closure id is BLAKE3 over the domain `active-sandbox-closure/v1`,
  a `0x00`, then length-prefixed document, source name, sorted include
  keys + digests, sorted env names + presence/value, and profile
  presence/name. It contains no host path, time, or process id and is
  the proof/memo token. The public `id` accessor is a commitment (a
  host may log it for equality); it is not a secret and must not be
  treated as one.
- The host profile selection must name a profile the document declares;
  otherwise `Sandbox::parse` fails with E048. The document's own
  `active_profile` selecting a declared profile stays allowed.

## 2. Pipeline

1. The host builds the closure from its own data (the only I/O point).
2. `InputClosure::new` validates and canonicalizes it.
3. `Sandbox::new` computes the closure id.
4. `Sandbox::parse` sets a closure-only include loader, disables the
   lockfile/store/parent escapes, parses the document text with a
   synthetic root, and validates the profile selection.
5. `Sandbox::evaluate*` clones the caller's engine, replaces its env
   with the closure's `Some` values, sets the closure profile, refuses a
   fallback or a differing host profile, installs a private guard, and
   calls the matching engine method.
6. `evaluate_walk` re-checks the guard: every `env` node must be a
   declared closure name and the engine's env map must equal the
   closure snapshot. The guard is `pub(crate)` and the wrapped module has
   no public constructor. `SandboxedModule::module` and `with_engine` are
   embedding helpers (LSP/graph/audit need the arena): the guarantee is
   that evaluations through `Sandbox` are guarded, not that malicious
   in-process code cannot extract the module and call a bare `Engine`.
   That extraction is outside the sandbox boundary and must not be used
   by a host to claim sandboxing.

Includes come only from the closure loader; the loader receives the
lexical include path, maps it to the root-relative key, and returns the
bytes or E048. The parser never falls back to a filesystem read.

## 2a. Manifest wire format (§7.2a)

The CLI, FFI, WASM, and LSP all accept one JSON object:

```
{
  "version": 1,                       // required, exactly 1
  "document_b64": "<base64>",         // optional if the surface supplies the document
  "source_name": "main.active",       // optional; diagnostic identity only
  "includes": [                        // optional array
    { "path": "lib.active",            // required, validated before any read
      "sha256": "<64 lowercase hex>",  // required
      "bytes_b64": "<base64>" }        // optional; otherwise the host reader supplies UTF-8
  ],
  "env": [                             // optional array
    { "name": "PORT",                  // required; folded uppercase
      "set": true,                     // optional bool, default false
      "value": "8080" }                // required iff set
  ],
  "profile": "prod"                    // optional string
}
```

Rules: base64 is standard with padding; unknown fields are rejected;
a present field of the wrong type is an error (never a fallback);
duplicate JSON/array keys are rejected; `sha256` must be ASCII 64-hex
and a non-ASCII digest is a construction error. Include keys reject
backslash, absolute/UNC/drive/ADS spellings, `..`, empty keys, control
characters, and raw spellings over 1024 bytes before the host reader is
called; `.` segments normalize away. The synthetic root basename is a
reserved identity, never an include. Duplicate normalized include keys or exact env names are
errors. The document and the sum of include bytes are capped at 16 MiB
each; a loader result over the remaining include budget is E028 before
lexing. A manifest that reaches the constructor with a wrong digest is
`ClosureError::IncludeDigest` (construction refusal); the loader
re-verifies bytes at parse time and surfaces a mismatch as E048. Env
names fold; duplicates after folding are errors. `profile` must name a
declared document profile or `Sandbox::parse` refuses E048. Per-surface
error classes: CLI exit 1 with the code in stderr; FFI
`ACTIVE_ERR_SANDBOX_REFUSED`; WASM envelope `E048` (or `PARSE` for a
malformed manifest); LSP `Diagnostic { code: "E048" }`.

A secret-analysis truncated profile is also refused as **E048**: the
profile hit its analysis bounds, so redaction cannot be honored (unknown
paths would over-redact). The closure cannot fix this by growing; a host
must raise the analysis scale or refuse the export. This is an E048
subcase, not a new code.

## 3. Determinism

For the same closure and engine configuration, a sandboxed run produces
the same value bytes, diagnostics (codes, paths, spans, order), and
content hash as an unsandboxed run with the same env/include/profile
inputs. Host paths in include metadata are normalized to closure
identities; diagnostics use the closure source name.

## 4. Budgets and platforms

Budgets are engine configuration, not closure input; the sandbox
neither raises nor lowers them. On wasm32 and Miri there is no wall
clock, so the sandbox claims only the deterministic count/depth/size
bounds plus host termination; it never claims a wall bound it cannot
enforce.

## 5. Config axes outside the closure

`Engine` policy that is a host decision (envelope gate, trusted keys,
budgets, reader version, blob salt, materialisation, purity) stays
engine configuration and is part of the evaluator tuple. A random
`blob_salt` remains a documented non-reproducible host choice.

## 6. Surfaces

`Sandbox` mirrors the `Engine` evaluation methods: `evaluate`,
`evaluate_with_seeds`, `evaluate_collecting_seeds`,
`evaluate_with_steps`, `evaluate_with_steps_and_seeds`,
`evaluate_incremental`, and `evaluate_incremental_with_seeds`. The
formatter never evaluates. A surface that evaluates outside this list
is unsandboxed by construction; adopting the sandbox means calling the
`Sandbox` method rather than the engine method.

**New-surface checklist (review question).** Before a surface is
accepted, answer: *does it evaluate?* If yes, name (a) the sandboxed
entry it calls, (b) where its `InputClosure` comes from (host data,
editor buffer, manifest, or FFI/WASM envelope), and (c) the smoke test
that proves an out-of-closure env/include/profile read is E048. A
surface that evaluates through a bare `Engine` is an unsandboxed legacy
surface and must say so in its docs; it cannot claim L-P28 support. The
CLI, test runner, FFI, WASM, LSP, bindings that expose those symbols,
and the playground inherit this rule.

## 7. Refusal code

**E048** is a sandbox refusal: an undeclared env read, an include not in
the closure or with a changed digest, a host profile outside the
closure, an environment fallback, or a fallback-bearing provider. The
message names the input family and never a value.

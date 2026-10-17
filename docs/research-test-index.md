# Research Test Index — YAML/TOML/JSON failure research → Active test backlog

Source corpus: `C:\Users\Usuario\Downloads\Solid Tests Research Work` (read-only).
Status legend: `pending` → `created` → `passing` / `fixed` (code changed to pass).
`fixed` entries must cite the fix commit. Never flip to passing by editing an
expectation: failures are resolved in language/Rust code against `spec/` intent.

Conventions: tests live in `active-core/tests/research_battery.rs` with stable
IDs `RB-001…` in doc comments; each maps to ≥1 index row below. Shipped entry
points only (`parse`/`parse_file` → `Engine::evaluate` → JSON export, or the
real `active` binary). Diagnostic classes are Active codes (`docs/diagnostics.md`),
not the research docs' proposed names.

## §A — 20 core contracts (`tests/curated/00-DERIVED-SOLID-Core-Contracts.md`)

| ID | Contract | Expected Active behavior | Status |
|---|---|---|---|
| DERIVED-0001 | Explicit `=`; colon pairs are specified (`value : 1` = key-value per dual-syntax.md:49) | Accept as pair; bodiless `key:` rejects E008 | passing (refusal pinned: colon pairs are specified) → Batch 1 RB-001 |
| DERIVED-0002 | Undefined `let` reference | W001 at name span; poisons arithmetic to T001 | passing → Batch 4 RB-039 |
| DERIVED-0003 | `let` cycles (2- and 3-node) | S001 cycle error | passing → Batch 3 RB-030 |
| DERIVED-0004 | Malformed interpolation delimiters | E012 unclosed / E013 empty, spans | passing → Batch 4 RB-040 |
| DERIVED-0005 | Unit dimension mismatch (`5s + 512MiB`) | Reject dimension mismatch | passing → Batch 2 RB-011 |
| DERIVED-0006 | Env fallback absent variable | Default returned | pending |
| DERIVED-0007 | Env empty vs absent distinct | `""` wins; absent falls back | passing → Batch 1 RB-007 |
| DERIVED-0008 | Secret taint in interpolation | Refused S004 (spec/secrets.md:27) | passing → Batch 1 RB-005 |
| DERIVED-0009 | Secret redaction in diagnostics | Fail without leaking bytes | passing → Batch 1 RB-006 |
| DERIVED-0010 | Profile precedence deterministic | Documented order wins, stable | passing → Batch 2 RB-016 |
| DERIVED-0011 | Profile inheritance cycle | Reject `E_PROFILE_CYCLE`-class | pending |
| DERIVED-0012 | `@remove` missing path | **W004 idempotent success** (Active intent, not error — diagnostics.md:91) | passing → Batch 2 RB-014 |
| DERIVED-0013 | `@rename` collision | Reject, no silent overwrite | passing → Batch 2 RB-015 |
| DERIVED-0014 | Patch preserves taint | Patched-in secret stays secret, export redacts | passing → Batch 3 RB-029 |
| DERIVED-0015 | Schema range boundary inclusive | Accept 65535; reject 0/65536 | passing → Batch 2 RB-012 |
| DERIVED-0016 | Missing vs null distinct codes | Missing ≠ null mismatch code | passing → Batch 2 RB-013 |
| DERIVED-0017 | Hermetic eval, no network | Zero network syscalls during eval | pending |
| DERIVED-0018 | Interpolation expansion budget | Abort with budget diagnostic, no hang | pending |
| DERIVED-0019 | Round-trip preserves units | `5s`/`512MiB` survive print→parse | pending |
| DERIVED-0020 | Dual-syntax semantic equality | Brace ≡ indent values | pending |

## §B — P0 safety (`tests/curated/01-P0-Safety-Resource-Limits.md`)

| ID | Contract | Expected Active behavior | Status |
|---|---|---|---|
| P0-001 | ≥300 nesting bounded | Budget/depth reject, no overflow | passing (budget trips with span) → Batch 1 RB-010 |
| P0-002 | 5000-key table bounded | Budget accept-or-reject, no hang | pending |
| P0-003 | Huge array bounded | Size budget enforced | pending |
| P0-004 | Interpolation expansion budget | Abort, no unbounded growth | pending |
| P0-005 | Simple `let` cycle | `E_DEPENDENCY_CYCLE` | pending |
| P0-006 | 4-chain cycle | `E_DEPENDENCY_CYCLE` | pending |
| P0-007 | Self-referential `let x = x` | S001 cycle | passing → Batch 4 RB-036 |
| P0-008 | Secret absent from diagnostics | No raw bytes in output | pending |
| P0-009 | Secret absent from JSON export | Redaction marker only | pending |
| P0-010 | Taint through interpolation | Tainted or rejected, export redacts | pending |
| P0-011 | No network during eval | Harness-observed zero I/O | pending |
| P0-012 | No ambient file reads w/o include | Filesystem untouched | pending |
| P0-013 | Include cycle A↔B | Include/cycle class reject | pending |
| P0-014 | Include path must be literal | Reject interpolated path | pending |
| P0-015 | Truncated input never panics | Controlled diagnostic | pending |
| P0-016 | Random binary never panics | Controlled reject | pending |
| P0-017 | Wall-clock budget aborts | Time-exceeded diagnostic | pending |
| P0-018 | No alias-style exponential expansion | Budget/cycle before materialize | pending |
| P0-019 | Duplicate diagnostics stable | Same class + spans across runs | pending |
| P0-020 | Control suite passes under budgets | Realistic config evaluates | pending |

## §C — P1 type surprises (`tests/curated/02-P1-Type-Inference-and-Surprises.md`)

| ID | Contract | Expected Active behavior | Status |
|---|---|---|---|
| P1-TI-001 | `"NO"` string; bare `NO` rejected/unresolved | No Norway coercion | pending |
| P1-TI-002 | `yes/no/on/off` quoted strings | Strings; bare forms not booleans | passing → Batch 2 RB-018 |
| P1-TI-003 | `"1.10"` keeps zeros | Exact strings, never floats | passing (exact strings) → Batch 1 RB-008 |
| P1-TI-004 | Leading-zero policy explicit | `0755` rejected or documented | pending |
| P1-TI-005 | `null` vs `"null"` | Null vs string | pending |
| P1-TI-006 | `true/false` vs `"true"`/`"false"` | Bool vs string; capitals documented | pending |
| P1-TI-007 | Empty vs null vs missing trio | Three distinct states | passing → Batch 2 RB-019 |
| P1-TI-008 | `10` int vs `10.0` float | Distinct types, intent kept | passing → Batch 2 RB-017 |
| P1-TI-009 | Underscore numbers explicit | `1_000_000` per grammar or reject | pending |
| P1-TI-010 | Keyword-lookalikes in arrays stay strings | No element coercion | pending |
| P1-TI-011 | Keyword-like keys policy | Allowed as keys or clear diagnostic | pending |
| P1-TI-012 | Duplicate keys explicit | **Last-wins + W011 warning** (reject would break existing last-wins tests) | fixed (W011 warning, last-wins kept) → Batch 1 RB-002 |
| P1-TI-013 | Key case sensitivity | Case-sensitive distinct keys | pending |
| P1-TI-014 | Unicode keys | Accept without corruption or clear reject | pending |
| P1-TI-015 | Positive control matrix | Exact types, no coercions | pending |

## §D — P1 structural (`tests/curated/03-P1-Structural-Syntax-Arrays-Tables.md`)

| ID | Contract | Expected Active behavior | Status |
|---|---|---|---|
| P1-ST-001 | `[1,,2]` rejects | Missing-element/unexpected-token class | pending |
| P1-ST-002 | `[1 2]` rejects | Missing-separator class | pending |
| P1-ST-003 | Unterminated array rejects | Unterminated class | pending |
| P1-ST-004 | Unterminated inline table rejects | Unterminated class | pending |
| P1-ST-005 | Trailing comma policy locked | H002 healed accept (observed) | passing (H002 healed accept) → Batch 1 RB-004 |
| P1-ST-006 | `[]` and `{}` accept | Empty containers | pending |
| P1-ST-007 | Nested arrays/tables of inline tables | Correct structure | pending |
| P1-ST-008 | `[[tables]]` support or clean reject | Per grammar, no half-behavior | pending |
| P1-ST-009 | Array/table same-key conflict | Structural-conflict diagnostic | pending |
| P1-ST-010 | Inline-then-block table conflict | Documented conflict behavior | pending |
| P1-ST-011 | Dotted keys ≡ nested tables (or not) | Locked semantics, dual forms | pending |
| P1-ST-012 | `[,]` / `[1,,]` reject | Reject | pending |
| P1-ST-013 | Bare word in array rejects | Unresolved identifier unless quoted | pending |
| P1-ST-014 | `[server host]` rejects | No spaces in headers | pending |
| P1-ST-015 | Positive structural matrix | Identical semantics incl. dual-syntax | pending |

## §E — P1 secrets/env/units (`tests/curated/04-P1-Secrets-Env-Taint-Units.md`)

| ID | Contract | Expected Active behavior | Status |
|---|---|---|---|
| P1-SE-001 | `secret("s3cr3t")` typed + redacted | Secret kind, export redacts | pending |
| P1-SE-002 | `secret(env.DB_PASSWORD)` | Env-backed secret, never dumped | pending |
| P1-SE-003 | Taint through table construction | Field stays tainted | pending |
| P1-SE-004 | Secret arithmetic rejected | Type/taint error, no leak | pending |
| P1-SE-005 | Missing vs empty env (suite form) | Same as 0006/0007 | pending |
| P1-SE-006 | Env fallback chain first-wins | Deterministic, empty-policy consistent | passing → Batch 3 RB-027 |
| P1-SE-007 | Time units normalize | Common duration representation | pending |
| P1-SE-008 | Byte units normalize | Common byte representation | pending |
| P1-SE-009 | Unknown unit explicit | E002 reject (known-suffix table only) | passing → Batch 4 RB-037 |
| P1-SE-010 | Unit on non-number rejects | E002 reject | passing → Batch 4 RB-038 |
| P1-SE-011 | Dimension mismatch (suite form) | Same as 0005 | pending |
| P1-SE-012 | Scalar unit multiply (`5s * 3`) | 15s normalized | pending |
| P1-SE-013 | `secret("5s")` opaque | Never auto-parsed as unit | passing → Batch 3 RB-028 |
| P1-SE-014 | Export redaction matrix | All positions × all formats redact | pending |
| P1-SE-015 | Realistic service config control | Evaluates, redacts, normalizes | pending |

## §F — P1 dual-syntax/diagnostics (`tests/curated/05-P1-Dual-Syntax-Roundtrip-Diagnostics.md`)

| ID | Contract | Expected Active behavior | Status |
|---|---|---|---|
| P1-DS-001 | Brace ≡ indent values | Structurally equal | pending |
| P1-DS-002 | Nested dual-syntax equality | Equal semantics | pending |
| P1-DS-003 | Mixed style legal-or-reject | Deterministic either way | pending |
| P1-DS-004 | Brace round-trip | Semantic equality | pending |
| P1-DS-005 | Indent round-trip | Semantic equality | pending |
| P1-DS-006 | Comments ignored semantically | Value `1` unaffected | passing → Batch 2 RB-020 |
| P1-DS-007 | Hash in string kept | `"abc # not comment"` intact | passing (hash kept, comments ignored) → Batch 1 RB-009 |
| P1-DS-008 | Every failure has spans | Usable line/column | pending |
| P1-DS-009 | Diagnostic class stable | Same class across runs + styles | pending |
| P1-DS-010 | Unresolved span covers name | Span on `unknown` | pending |
| P1-DS-011 | Units survive round-trip | Duration value kept | pending |
| P1-DS-012 | Secrets redacted across print | No raw bytes in printer output | pending |
| P1-DS-013 | Canonical key order deterministic | Documented order | pending |
| P1-DS-014 | Triple-form matrix | Equal values + round-trip | pending |
| P1-DS-015 | Colon variants reject uniformly | Same class as 0001 | passing (refusal pinned: colon pairs are specified) → Batch 1 RB-001 |

## §G — Resilience report, 75 HIST items (`Architecting Resilience…md`)

Severity `critical` first. Source section cited per row (§1 parser robustness,
§2 DoS, §3 deserialization).

| ID | Title | Source § | Expected Active behavior | Status |
|---|---|---|---|---|
| HIST-YAML-004 | Duplicate key last-wins | §1 | Deterministic last-wins + W011 | fixed (W011 warning, last-wins kept) → RB-002 |
| HIST-YAML-006 | Duplicate key rejection (strict) | §1 | **Not adopted**: Active warns, keeps last-wins (existing tests depend on it) | pending (documents rejection of the alternative) |
| HIST-YAML-007 | Merge-key quadratic DoS | §2 | N/A: no merge keys; regression-guard the absence | passing → Batch 4 RB-031 |
| HIST-YAML-008 | Merge-key exponential expansion | §2 | N/A: no merge keys; guard the absence | pending |
| HIST-YAML-009 | Alias bomb (collections) | §2 | N/A: no aliases; budget/cycle guard | passing → Batch 4 RB-032 |
| HIST-YAML-010 | Alias bomb (scalars) | §2 | N/A: no aliases; metadata-budget guard | pending |
| HIST-YAML-016 | Anchor cycle infinite loop | §2 | N/A: no anchors; cycle class exists | pending |
| HIST-YAML-003 | Empty key handling | §1 | Accept `""` key, round-trip exact | pending |
| HIST-YAML-011 | Multiline quotes preserved | §1 | Embedded quotes kept + round-trip | pending |
| HIST-YAML-013 | Comment-structure non-interference | §1 | Comments never alter structure | pending |
| HIST-YAML-014 | CRLF normalization | §1 | `\r\n` → single newline, deterministic print | pending |
| HIST-YAML-015 | Invalid UTF-8/surrogates rejected | §1 | Reject with Unicode class | pending |
| HIST-YAML-019 | Brace/indent mix consistency | §1 | Defined merge-or-error rule | passing (refusal pinned: colon pairs are specified) |
| HIST-YAML-020 | Whitespace in keys/values kept | §1 | Exact preservation + round-trip | pending |
| HIST-YAML-021 | BOM skipped | §1 | Skip leading U+FEFF | fixed (BOM skipped in lexer) → RB-003 |
| HIST-YAML-022 | Trailing comma accepted | §1 | H002 healed accept (observed) | passing (H002 healed accept) → RB-004 |
| HIST-YAML-024 | Long float literals | §1 | Accurate f64 conversion, no crash | pending |
| HIST-YAML-025 | Scientific notation edges | §1 | Per f64 grammar or clean reject | pending |
| HIST-JSON-001 | Deep nesting stack overflow | §2 | Hard depth limit, diagnostic not crash | passing (budget trips with span) → RB-010 |
| HIST-JSON-002 | No internal exception types leak | §2 | Documented error classes only | pending |
| HIST-JSON-003 | Fail fast before memory exhaustion | §2 | Reject long before RAM pressure | pending |
| HIST-JSON-004 | Large integer precision | §2 | E023 saturation path (exists); pin boundary | pending |
| HIST-JSON-005 | Exponent under/overflow | §2 | IEEE 754 policy for f64 surface | pending |
| HIST-TOML-001 | Inline nesting depth limit | §2 | Same limit as arrays | pending |
| HIST-TOML-002 | No raw RangeError-class leaks | §2 | Documented classes only (Rust panics = bugs) | passing → Batch 4 RB-033 |
| HIST-TOML-003 | Inline table nesting limit | §2 | Depth limit applies | pending |
| HIST-TOML-004 | Long dotted-key recursion | §2 | Graph build protected | passing → Batch 4 RB-034 |
| HIST-TOML-005 | Comment-EOF infinite loop | §2 | Terminates (timeout/budget guard evidence) | passing → Batch 3 RB-022 |
| HIST-TOML-006 | Stringifier cycle guard | §2 | Formatter terminates on cyclic IR | pending |
| HIST-TOML-007 | Array-of-tables growth bound | §2 | Array size budget enforced | pending |
| HIST-TOML-008 | Non-UTC datetime offsets | §2 | N/A: no datetimes; importer stringifies + W008 | pending |
| HIST-TOML-00B | Trailing comma policy | §2 | Same as HIST-YAML-022 | pending |
| HIST-TOML-00C | Duplicate keys last-wins | §2 | Same as HIST-YAML-004 | pending |
| HIST-TOML-00D | Non-string table key rejected | §2 | Keys are identifiers/strings only | pending |
| HIST-TOML-00E | Unclosed string diagnostic | §2 | Clear unterminated-string error | pending |
| HIST-TOML-00F | Bad escape rejected | §2 | Invalid-escape diagnostic | pending |
| HIST-TOML-00K | Serializer cycle guard | §2 | Same as HIST-TOML-006 | pending |
| HIST-TOML-00L | `__proto__` neutral key | §3 | Plain key, no special meaning | passing → Batch 3 RB-026 |
| HIST-TOML-00M | Secrets absent from error output | §3 | Sanitized diagnostics (Vault CVE-2025-4166 class) | passing → Batch 3 RB-025 |
| HIST-TOML-00N | Table-merge round-trip | §2 | Merged structure reprints equivalently | pending |
| HIST-TOML-00P | Comments survive round-trip | §2 | Adjacent comments kept, data identical | pending |
| HIST-YAML-026 | Unsafe deserialization RCE (py) | §3 | N/A: no tags/constructors; reject `!` tags | passing → Batch 3 RB-023 |
| HIST-YAML-027 | Unsafe deserialization RCE (java) | §3 | N/A: no class system; reject | passing → Batch 3 RB-023 |
| HIST-YAML-028 | Unsafe deserialization RCE (js) | §3 | N/A: hermetic eval, no host calls | passing → Batch 3 RB-023 |
| HIST-YAML-029 | No safe/unsafe loader split | §3 | Single secure mode; no unsafe flag exists | pending |
| HIST-YAML-030 | Custom constructor abuse | §3 | No user-defined constructors invoked | pending |
| HIST-YAML-031 | `__proto__` parses as plain key | §3 | Same as HIST-TOML-00L | passing → Batch 3 RB-026 |
| HIST-YAML-036 | SSTI: `{{7*7}}` stays literal | §3 | Interpolation is `${…}`-only; braces literal | passing → Batch 3 RB-024 |
| HIST-YAML-037 | ReDoS via schema regex | §3 | Schema patterns bounded or engine-safe | pending |
| HIST-YAML-040 | Error output sanitized | §3 | Same as HIST-TOML-00M | passing → Batch 3 RB-025 |
| HIST-YAML-041 | Include path sandbox | §3 | Literal/relative paths only, traversal rejected | pending |
| HIST-YAML-042 | Dup-key parser differential | §3 | Single deterministic policy everywhere | pending |
| HIST-YAML-001 | Multiline comma in array | §1 | Comma-in-string vs separator distinguished | pending |
| HIST-YAML-002 | Complex keys rejected | §1 | Only scalar keys; clear diagnostic | pending |
| HIST-YAML-005 | First-wins alternative | §1 | **Not adopted**: documents why last-wins won | pending |
| HIST-YAML-012 | Chomping behavior | §1 | N/A: no chomping indicators; triple-quote rules pinned | pending |
| HIST-YAML-017 | Anchor collision | §1 | N/A: no anchors; `let` redefinition policy pinned instead | pending |
| HIST-YAML-018 | Merge-key type error | §1 | N/A: no merge keys | pending |
| HIST-YAML-023 | Self-reference via anchors | §1 | N/A: `let` self-reference cycle class instead | pending |
| HIST-TOML-009 | Ambiguous local time | §2 | N/A: no datetimes (importer strings) | pending |
| HIST-TOML-00A | Skipped local time | §2 | N/A: no datetimes (importer strings) | pending |
| HIST-TOML-00G | Large integer parse time | §2 | Linear-time digit handling | pending |
| HIST-TOML-00H | Long datetime parse time | §2 | N/A: no datetimes | pending |
| HIST-TOML-00I | Max line length | §2 | Documented: bounded or explicitly unbounded | pending |
| HIST-TOML-00J | Max document size | §2 | Documented input-size posture | passing → Batch 4 RB-035 |
| HIST-TOML-00O | Datetime round-trip | §2 | N/A: datetimes are strings | pending |
| HIST-YAML-032 | Merge taint | §3 | N/A: no merge operator | pending |
| HIST-YAML-033 | Deep-copy taint | §3 | Clone paths keep `Secret` kind | pending |
| HIST-YAML-034 | Spread taint | §3 | N/A: no spread operator | pending |
| HIST-YAML-035 | Dynamic-access taint | §3 | Indexing a secret path stays secret-typed or refused | pending |
| HIST-YAML-038 | ReDoS in evaluation | §3 | No regex builtins; interpolation bounded | pending |
| HIST-YAML-039 | SSRF path traversal | §3 | String concat is literal; no fetch builtins | pending |
| HIST-YAML-043 | Boolean determinism | §3 | One boolean spelling, one value everywhere | pending |
| HIST-YAML-044 | URL backslash ambiguity | §3 | N/A: no URL parser in core | pending |
| HIST-YAML-045 | Signature/duplicate-key split | §3 | Same as HIST-YAML-042 + sign.rs coverage | pending |

## §H — Catalogue families (`SOLID-Historical-Test-Catalogue-1000plus.md`)

| Family | IDs | Mining note | Status |
|---|---|---|---|
| SRC-TOML (95) | `SRC-TOML-0001`…`0095` | Upstream toml-test invalid fixtures → Active analogues (reject classes, escapes, tables) | pending (mine per batch) |
| SRC-JSON (34) | `SRC-JSON-0001`…`0034` | JSONTestSuite edges → Active analogues (nesting, numbers, escapes, dup keys) | pending (mine per batch) |
| SRC-YAML (15) | `SRC-YAML-0001`…`0015` | yaml-test-suite edges → Active analogues | pending (mine per batch) |
| SRC-CVE (10) | `SRC-CVE-0001`…`0010` | CVE-derived payloads → budget/cycle/sanitization analogues | pending (mine per batch) |
| SRC-SPEC (10) | `SRC-SPEC-0001`…`0010` | Standards-spec edges → grammar conformance analogues | pending (mine per batch) |
| Byte/token fixtures | `#` headings (~117) | Raw-byte edges (CR/NUL/TAB/controls); byte-exact inputs live beside tests | pending (mine per batch) |

## §I — PDF + Rust guide inputs

- `20 Derived Solid tests from the 1000 originals.pdf`: scanned summary; content duplicates §A (verified p1/p5). Canonical reference stays §A.
- `From Syntax to Security…md` hardening backlog (build when ideas strike, even out of scope):
  - H-01 `unsafe`-footprint audit (`cargo geiger`-style inventory + justification per site).
  - H-02 Miri run over parser/evaluator unit tests in CI (nightly gate).
  - H-03 Differential fuzzing: Active parser vs TOML/JSON parsers on shared shapes.
  - H-04 `cargo-deny` policy (licenses, duplicate deps, banned crates).
  - H-05 Structure-aware fuzzing via `arbitrary` on the Active AST.
  - H-06 Grammar-generated corpus (production-rule exhaustive cases, bounded depth).

## Batches ledger

| Batch | Tests | Status |
|---|---|---|
| Batch 1 | RB-001…RB-010 (§A/§C/§G rows marked) | done: RB-001 refusal pinned, W011 + BOM fixes, rest passing |
| Batch 2 | RB-011…RB-020 (§A/§C rows marked) | done: all passing; RB-015 needed two expectation corrections from shipped syntax (plain-name rename targets) |
| Batch 3 | RB-021…RB-030 (HIST-JSON-001, HIST-TOML-005, HIST-YAML-026/027/028/036, HIST-TOML-00L/00M, DERIVED-0003/0014, P1-SE-006/013) | done: 9 green first-try; RB-030 needed one test-side correction (lets don't export — read via key) |
| Batch 4 | RB-031…RB-040 (HIST-YAML-007/009, HIST-TOML-002/004/00J, P0-007, P1-SE-009/010, DERIVED-0002/0004) | done: 8 green first-try; RB-031 control + RB-039/RB-040 corrections from shipped behavior |
| Batch 5+ | Next 10 lowest pending rows by severity, per the loop prompt | pending |

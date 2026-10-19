#| Batch 152 | RB-1509…RB-1518 (SYN-0133-05…09, SYN-0134-01…05) | done: all 10 green first-try, no code changes; 133 continued (brace-dup W011 2:29; TAB E006 2:1; LF escape; mismatch E001 2:24; comment inert); 134 opened as 131 twins |
| Batch 153 | RB-1519…RB-1528 (SYN-0134-06…09, SYN-0135-01…06) | done: all 10 green first-try, no code changes; 134 completed as 131 twins; 135 opened as 124 twins (flat/nested W011 refusals; quoted E001 1:18; distinct silent; U+00E9 E001) |
| Batch 154 | RB-1529…RB-1538 (SYN-0135-07…09, SYN-0136-01…07) | done: all 10 green first-try, no code changes; 135 completed (patch/remove E019 3:2; dotted E001 1:18); 136 opened as 133 twins (colon-twin equality; dup W011; TAB E006; LF) |
| Batch 155 | RB-1539…RB-1548 (SYN-0136-08…09, SYN-0137-01…08) | done: all 10 green first-try, no code changes; 136 completed (mismatch E001 2:24; comment inert); 137 opened as 133 twins (colon-twin equality; dup W011; TAB E006; LF; mismatch) |
| Batch 156 | RB-1549…RB-1558 (SYN-0137-09, SYN-0138-01…09) | done: all 10 green first-try, no code changes; 137 completed (comment inert); 138 opened as 133 twins (colon-twin equality; dup W011; TAB E006; LF; mismatch; comment) |
| Batch 157 | RB-1559…RB-1568 (SYN-0139-01…09, SYN-0140-01) | done: all 10 green first-try, no code changes; 139 opened as Norway twins (yes/no/on/off W001 1:16 refusals; true/false; FALSE W001; null; quoted-no; 1.0 float); 140 opened |
| Batch 158 | RB-1569…RB-1578 (SYN-0140-02…09, SYN-0141-01…02) | done: all 10 green first-try, no code changes; 140 continued as 133 twins (colon-twin equality; dup W011; TAB E006; LF; mismatch; comment); 141 opened (baseline + nested) |
| Batch 159 | RB-1579…RB-1588 (SYN-0141-03…09, SYN-0142-01…03) | done: all 10 green first-try, no code changes; 141 continued as 131 twins; 142 opened (brace/equals-refusal/nested) |
| Batch 160 | RB-1589…RB-1598 (SYN-0142-04…09, SYN-0143-01…04) | done: all 10 green first-try, no code changes; 142 continued as 133 twins; 143 opened (brace/equals-refusal/nested/nested-refusal) |
| Batch 161 | RB-1599…RB-1608 (SYN-0143-05…09, SYN-0144-01…05) | done: all 10 green first-try, no code changes; 143 continued as 133 twins; 144 opened (baseline/nested/array/string/comment); one transient rb_086 flake, green on re-run (2234) |
 Research Test Index — YAML/TOML/JSON failure research → Active test backlog

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
| DERIVED-0006 | Env fallback absent variable | Default returned | passing → Batch 6 RB-060 |
| DERIVED-0007 | Env empty vs absent distinct | `""` wins; absent falls back | passing → Batch 1 RB-007 |
| DERIVED-0008 | Secret taint in interpolation | Refused S004 (spec/secrets.md:27) | passing → Batch 1 RB-005 |
| DERIVED-0009 | Secret redaction in diagnostics | Fail without leaking bytes | passing → Batch 1 RB-006 |
| DERIVED-0010 | Profile precedence deterministic | Documented order wins, stable | passing → Batch 2 RB-016 |
| DERIVED-0011 | Profile inheritance cycle | No inheritance feature: `inherits` merges as plain data, nothing to cycle | passing → Batch 7 RB-065 |
| DERIVED-0012 | `@remove` missing path | **W004 idempotent success** (Active intent, not error — diagnostics.md:91) | passing → Batch 2 RB-014 |
| DERIVED-0013 | `@rename` collision | Reject, no silent overwrite | passing → Batch 2 RB-015 |
| DERIVED-0014 | Patch preserves taint | Patched-in secret stays secret, export redacts | passing → Batch 3 RB-029 |
| DERIVED-0015 | Schema range boundary inclusive | Accept 65535; reject 0/65536 | passing → Batch 2 RB-012 |
| DERIVED-0016 | Missing vs null distinct codes | Missing ≠ null mismatch code | passing → Batch 2 RB-013 |
| DERIVED-0017 | Hermetic eval, no network | Zero network syscalls during eval | passing → Batch 7 RB-066 (no network-capable deps in lockfile) |
| DERIVED-0018 | Interpolation expansion budget | Abort with budget diagnostic, no hang | passing → Batch 10 RB-093 (geometric growth trips X) |
| DERIVED-0019 | Round-trip preserves units | `5s`/`512MiB` survive fmt→parse→fmt | passing → Batch 8 RB-077 |
| DERIVED-0020 | Dual-syntax semantic equality | Brace ≡ indent values | passing → Batch 11 RB-102 |

## §B — P0 safety (`tests/curated/01-P0-Safety-Resource-Limits.md`)

| ID | Contract | Expected Active behavior | Status |
|---|---|---|---|
| P0-001 | ≥300 nesting bounded | Budget/depth reject, no overflow | passing (budget trips with span) → Batch 1 RB-010 |
| P0-002 | 5000-key table bounded | Budget accept-or-reject, no hang | passing → Batch 9 RB-086 (evaluates) |
| P0-003 | Huge array bounded | Size budget enforced | passing → Batch 9 RB-087 (X001 trip) |
| P0-004 | Interpolation expansion budget | Abort, no unbounded growth | passing (covered: RB-093 geometric X-trip + RB-032) |
| P0-005 | Simple `let` cycle | S001 (E_DEPENDENCY_CYCLE class) | passing (covered: RB-030 2-node) |
| P0-006 | 4-chain cycle | S001 (E_DEPENDENCY_CYCLE class) | passing (covered: RB-030 3-node) |
| P0-007 | Self-referential `let x = x` | S001 cycle | passing → Batch 4 RB-036 |
| P0-008 | Secret absent from diagnostics | No raw bytes in output | passing (covered: Batch 1 RB-005 S004 + RB-006 redaction) |
| P0-009 | Secret absent from JSON export | Redaction marker only | passing (covered: Batch 1 RB-006) |
| P0-010 | Taint through interpolation | Tainted or rejected, export redacts | passing (covered: Batch 1 RB-005 S004 refusal) |
| P0-011 | No network during eval | Harness-observed zero I/O; lockfile has no network-capable crates | passing → Batch 7 RB-066 |
| P0-012 | No ambient file reads w/o include | Filesystem untouched | passing → Batch 12 RB-111 (document-relative E017 + sibling merge) |
| P0-013 | Include cycle A↔B | Include/cycle class reject | passing (covered: includes.rs E016 circular-include test; spec/grammar.md:265) |
| P0-014 | Include path must be literal | Reject interpolated path | passing → Batch 5 RB-048 |
| P0-015 | Truncated input never panics | Controlled diagnostic | passing → Batch 10 RB-091 (cut battery terminates) |
| P0-016 | Random binary never panics | Controlled reject | passing → Batch 10 RB-092 (hostile corpus) |
| P0-017 | Wall-clock budget aborts | Time-exceeded diagnostic | passing (covered: budgets.rs wall_clock_backstop_trips, X002) |
| P0-018 | No alias-style exponential expansion | Budget/cycle before materialize | passing (covered: Batch 10 RB-093 geometric X-trip + RB-032) |
| P0-019 | Duplicate diagnostics stable | Same class + spans across runs | passing → Batch 6 RB-059 |
| P0-020 | Control suite passes under budgets | Realistic config evaluates | passing → Batch 9 RB-088 |

## §C — P1 type surprises (`tests/curated/02-P1-Type-Inference-and-Surprises.md`)

| ID | Contract | Expected Active behavior | Status |
|---|---|---|---|
| P1-TI-001 | `"NO"` string; bare `NO` rejected/unresolved | No Norway coercion | passing → Batch 11 RB-103 |
| P1-TI-002 | `yes/no/on/off` quoted strings | Strings; bare forms not booleans | passing → Batch 2 RB-018 |
| P1-TI-003 | `"1.10"` keeps zeros | Exact strings, never floats | passing (exact strings) → Batch 1 RB-008 |
| P1-TI-004 | Leading-zero policy explicit | `0755` rejected or documented | passing → Batch 11 RB-104 (plain decimal 755) |
| P1-TI-005 | `null` vs `"null"` | Null vs string | passing → Batch 11 RB-105 |
| P1-TI-006 | `true/false` vs `"true"`/`"false"` | Bool vs string; capitals documented | passing → Batch 11 RB-106 |
| P1-TI-007 | Empty vs null vs missing trio | Three distinct states | passing → Batch 2 RB-019 |
| P1-TI-008 | `10` int vs `10.0` float | Distinct types, intent kept | passing → Batch 2 RB-017 |
| P1-TI-009 | Underscore numbers explicit | `1_000_000` per grammar or reject | passing → Batch 11 RB-107 (stripped to 1000000) |
| P1-TI-010 | Keyword-lookalikes in arrays stay strings | No element coercion | passing → Batch 13 RB-121 (quoted exact; bare W001+null, never coerced) |
| P1-TI-011 | Keyword-like keys policy | Statement-initial value tokens fail E002; mid-statement strays keep §4.1 skip | passing → Batch 11 RB-101 |
| P1-TI-012 | Duplicate keys explicit | **Last-wins + W011 warning** (reject would break existing last-wins tests) | fixed (W011 warning, last-wins kept) → Batch 1 RB-002 |
| P1-TI-013 | Key case sensitivity | Case-sensitive distinct keys | passing → Batch 11 RB-108 |
| P1-TI-014 | Unicode keys | Accept without corruption or clear reject | passing → Batch 11 RB-109 (E005 reject) |
| P1-TI-015 | Positive control matrix | Exact types, no coercions | passing → Batch 11 RB-110 (10-kind matrix) |

## §D — P1 structural (`tests/curated/03-P1-Structural-Syntax-Arrays-Tables.md`)

| ID | Contract | Expected Active behavior | Status |
|---|---|---|---|
| P1-ST-001 | `[1,,2]` rejects | H002 heals accept (stray-comma policy) | passing → Batch 9 RB-089 |
| P1-ST-002 | `[1 2]` rejects | Missing-separator class | passing → Batch 9 RB-090 |
| P1-ST-003 | Unterminated array rejects | Unterminated class | passing → Batch 12 RB-112 (E001, span 2:1) |
| P1-ST-004 | Unterminated inline table rejects | Unterminated class | passing → Batch 12 RB-113 (E001, span 2:1) |
| P1-ST-005 | Trailing comma policy locked | H002 healed accept (observed) | passing (H002 healed accept) → Batch 1 RB-004 |
| P1-ST-006 | `[]` and `{}` accept | Empty containers | passing → Batch 12 RB-114 |
| P1-ST-007 | Nested arrays/tables of inline tables | Correct structure | passing → Batch 12 RB-115 |
| P1-ST-008 | `[[tables]]` support or clean reject | Per grammar, no half-behavior | passing (covered: Batch 12 RB-118 pins clean E001 reject) |
| P1-ST-009 | Array/table same-key conflict | Structural-conflict diagnostic | passing → Batch 12 RB-116 (W011 last-wins, wholesale) |
| P1-ST-010 | Inline-then-block table conflict | Documented conflict behavior | passing → Batch 12 RB-117 (W011 last-wins, wholesale) |
| P1-ST-011 | Dotted keys ≡ nested tables (or not) | Locked semantics, dual forms | passing → Batch 12 RB-118 (`[a.b]` ≡ inline nest; dotted-key E002) |
| P1-ST-012 | `[,]` / `[1,,]` reject | Reject | passing (refusal pinned: H002 heals per locked stray-comma policy RB-089/RB-004) → Batch 12 RB-119 |
| P1-ST-013 | Bare word in array rejects | Unresolved identifier unless quoted | passing → Batch 12 RB-120 (W001 + null, quoted stays string) |
| P1-ST-014 | `[server host]` rejects | No spaces in headers | passing → Batch 13 RB-122 (E001+E002) |
| P1-ST-015 | Positive structural matrix | Identical semantics incl. dual-syntax | passing → Batch 13 RB-123 |

## §E — P1 secrets/env/units (`tests/curated/04-P1-Secrets-Env-Taint-Units.md`)

| ID | Contract | Expected Active behavior | Status |
|---|---|---|---|
| P1-SE-001 | `secret("s3cr3t")` typed + redacted | Secret kind, export redacts | passing → Batch 13 RB-124 (RAW marker absent everywhere) |
| P1-SE-002 | `secret(env.DB_PASSWORD)` | Env-backed secret, never dumped | passing → Batch 13 RB-125 (set + unset redact) |
| P1-SE-003 | Taint through table construction | Field stays tainted | passing → Batch 13 RB-126 |
| P1-SE-004 | Secret arithmetic rejected | Type/taint error, no leak | passing → Batch 13 RB-127 (T001, message redacted) |
| P1-SE-005 | Missing vs empty env (suite form) | Same as 0006/0007 | passing → Batch 13 RB-128 (suite form of RB-007/RB-060) |
| P1-SE-006 | Env fallback chain first-wins | Deterministic, empty-policy consistent | passing → Batch 3 RB-027 |
| P1-SE-007 | Time units normalize | Common duration representation | passing → Batch 13 RB-129 (ms map) |
| P1-SE-008 | Byte units normalize | Common byte representation | passing → Batch 13 RB-130 (integer bytes) |
| P1-SE-009 | Unknown unit explicit | E002 reject (known-suffix table only) | passing → Batch 4 RB-037 |
| P1-SE-010 | Unit on non-number rejects | E002 reject | passing → Batch 4 RB-038 |
| P1-SE-011 | Dimension mismatch (suite form) | Same as 0005 | passing → Batch 14 RB-131 (T001 suite form of RB-011) |
| P1-SE-012 | Scalar unit multiply (`5s * 3`) | 15s normalized | passing → Batch 14 RB-132 (ms 15000) |
| P1-SE-013 | `secret("5s")` opaque | Never auto-parsed as unit | passing → Batch 3 RB-028 |
| P1-SE-014 | Export redaction matrix | All positions × all formats redact | passing → Batch 14 RB-133 (top/nest/array × JSON/YAML/TOML/diags) |
| P1-SE-015 | Realistic service config control | Evaluates, redacts, normalizes | passing → Batch 14 RB-134 |

## §F — P1 dual-syntax/diagnostics (`tests/curated/05-P1-Dual-Syntax-Roundtrip-Diagnostics.md`)

| ID | Contract | Expected Active behavior | Status |
|---|---|---|---|
| P1-DS-001 | Brace ≡ indent values | Structurally equal | passing → Batch 14 RB-135 |
| P1-DS-002 | Nested dual-syntax equality | Equal semantics | passing → Batch 14 RB-136 |
| P1-DS-003 | Mixed style legal-or-reject | Deterministic either way | passing → Batch 14 RB-137 (legal + deterministic) |
| P1-DS-004 | Brace round-trip | Semantic equality | passing → Batch 14 RB-138 (model equality; order is DS-013's domain) |
| P1-DS-005 | Indent round-trip | Semantic equality | passing → Batch 14 RB-139 |
| P1-DS-006 | Comments ignored semantically | Value `1` unaffected | passing → Batch 2 RB-020 |
| P1-DS-007 | Hash in string kept | `"abc # not comment"` intact | passing (hash kept, comments ignored) → Batch 1 RB-009 |
| P1-DS-008 | Every failure has spans | Usable line/column | passing → Batch 10 RB-100 (failure sweep, all spanned) |
| P1-DS-009 | Diagnostic class stable | Same class across runs + styles | passing → Batch 14 RB-140 (W001 + name spans, flat and nested) |
| P1-DS-010 | Unresolved span covers name | Span on `unknown` | passing → Batch 15 RB-141 (W001 span 1:5, non-empty) |
| P1-DS-011 | Units survive round-trip | Duration value kept | passing → Batch 15 RB-142 (brace + indent round-trips) |
| P1-DS-012 | Secrets redacted across print | No raw bytes in printer output | passing (refusal pinned: fmt re-emits secret() source calls per spec/secrets.md:48-49; values never printed) → Batch 15 RB-143 |
| P1-DS-013 | Canonical key order deterministic | Documented order | passing → Batch 15 RB-144 (document order z,a,m; byte-stable) |
| P1-DS-014 | Triple-form matrix | Equal values + round-trip | passing → Batch 15 RB-145 |
| P1-DS-015 | Colon variants reject uniformly | Same class as 0001 | passing (refusal pinned: colon pairs are specified) → Batch 1 RB-001 |

## §G — Resilience report, 75 HIST items (`Architecting Resilience…md`)

Severity `critical` first. Source section cited per row (§1 parser robustness,
§2 DoS, §3 deserialization).

| ID | Title | Source § | Expected Active behavior | Status |
|---|---|---|---|---|
| HIST-YAML-004 | Duplicate key last-wins | §1 | Deterministic last-wins + W011 | fixed (W011 warning, last-wins kept) → RB-002 |
| HIST-YAML-006 | Duplicate key rejection (strict) | §1 | **Not adopted**: Active warns, keeps last-wins (existing tests depend on it) | passing (refusal pinned: nested-inline dup W011 last-wins) → Batch 16 RB-149 |
| HIST-YAML-007 | Merge-key quadratic DoS | §2 | N/A: no merge keys; regression-guard the absence | passing → Batch 4 RB-031 |
| HIST-YAML-008 | Merge-key exponential expansion | §2 | N/A: no merge keys; guard the absence | passing → Batch 5 RB-041 |
| HIST-YAML-009 | Alias bomb (collections) | §2 | N/A: no aliases; budget/cycle guard | passing → Batch 4 RB-032 |
| HIST-YAML-010 | Alias bomb (scalars) | §2 | N/A: no aliases; metadata-budget guard | passing → Batch 5 RB-042 |
| HIST-YAML-016 | Anchor cycle infinite loop | §2 | N/A: no anchors; cycle class exists | passing → Batch 7 RB-061 |
| HIST-YAML-003 | Empty key handling | §1 | Accept `""` key, round-trip exact | REFUSED: `""` is not an identifier (E002, same fix as -020) | passing → Batch 6 RB-052 |
| HIST-YAML-011 | Multiline quotes preserved | §1 | Embedded quotes kept + round-trip | passing → Batch 8 RB-080 |
| HIST-YAML-013 | Comment-structure non-interference | §1 | Comments never alter structure | passing → Batch 10 RB-094 (adversarial comments inert) |
| HIST-YAML-014 | CRLF normalization | §1 | `\r\n` → single newline, deterministic print | passing → Batch 8 RB-079 (fixed: lexer normalizes CR) |
| HIST-YAML-015 | Invalid UTF-8/surrogates rejected | §1 | Reject with Unicode class | REFUSED as specified: `\u` stays verbatim (grammar.md:64), surrogates can never materialize | passing → Batch 6 RB-054 |
| HIST-YAML-019 | Brace/indent mix consistency | §1 | Defined merge-or-error rule | passing (refusal pinned: colon pairs are specified) |
| HIST-YAML-020 | Whitespace in keys/values kept | §1 | Exact preservation + round-trip | REFUSED: keys are identifiers only (grammar E002); silent drop fixed → loud reject | passing → Batch 6 RB-051 |
| HIST-YAML-021 | BOM skipped | §1 | Skip leading U+FEFF | fixed (BOM skipped in lexer) → RB-003 |
| HIST-YAML-022 | Trailing comma accepted | §1 | H002 healed accept (observed) | passing (H002 healed accept) → RB-004 |
| HIST-YAML-024 | Long float literals | §1 | Accurate f64 conversion, no crash | passing → Batch 6 RB-058 |
| HIST-YAML-025 | Scientific notation edges | §1 | Per f64 grammar or clean reject | passing (covered: RB-053 exponents reject E002) |
| HIST-JSON-001 | Deep nesting stack overflow | §2 | Hard depth limit, diagnostic not crash | passing (budget trips with span) → RB-010 |
| HIST-JSON-002 | No internal exception types leak | §2 | Documented error classes only | passing (covered: RB-033 deep-input class audit) |
| HIST-JSON-003 | Fail fast before memory exhaustion | §2 | Reject long before RAM pressure | passing → Batch 5 RB-044 |
| HIST-JSON-004 | Large integer precision | §2 | E023 saturation path (exists); pin boundary | passing → Batch 9 RB-083 (u64::MAX) |
| HIST-JSON-005 | Exponent under/overflow | §2 | IEEE 754 policy for f64 surface | REFUSED as specified: no exponent grammar (E002 reject) | passing → Batch 6 RB-053 |
| HIST-TOML-001 | Inline nesting depth limit | §2 | Same limit as arrays | passing (covered: RB-045 200-deep inline tables) |
| HIST-TOML-002 | No raw RangeError-class leaks | §2 | Documented classes only (Rust panics = bugs) | passing → Batch 4 RB-033 |
| HIST-TOML-003 | Inline table nesting limit | §2 | Depth limit applies | passing → Batch 5 RB-045 |
| HIST-TOML-004 | Long dotted-key recursion | §2 | Graph build protected | passing → Batch 4 RB-034 |
| HIST-TOML-005 | Comment-EOF infinite loop | §2 | Terminates (timeout/budget guard evidence) | passing → Batch 3 RB-022 |
| HIST-TOML-006 | Stringifier cycle guard | §2 | Formatter terminates on cyclic IR | passing → Batch 5 RB-043 |
| HIST-TOML-007 | Array-of-tables growth bound | §2 | No `[[t]]` syntax: clean E001 reject; arrays of inline tables are the Active shape | passing → Batch 5 RB-046 |
| HIST-TOML-008 | Non-UTC datetime offsets | §2 | N/A: no datetimes; importer stringifies + W008 | passing → Batch 8 RB-078 |
| HIST-TOML-00B | Trailing comma policy | §2 | Same as HIST-YAML-022 | passing (covered: RB-004 H002) |
| HIST-TOML-00C | Duplicate keys last-wins | §2 | Same as HIST-YAML-004 | passing (covered: RB-002 W011 + last-wins) |
| HIST-TOML-00D | Non-string table key rejected | §2 | Keys are identifiers/strings only | passing → Batch 5 RB-047 |
| HIST-TOML-00E | Unclosed string diagnostic | §2 | H001 heal + span recorded (specified repair, not a failure) | passing → Batch 5 RB-049 |
| HIST-TOML-00F | Bad escape rejected | §2 | Verbatim passthrough per grammar.md:64-66 (refusal pinned) | passing → Batch 5 RB-050 |
| HIST-TOML-00K | Serializer cycle guard | §2 | Same as HIST-TOML-006 | passing (covered: RB-043 formatter-on-cycles) |
| HIST-TOML-00L | `__proto__` neutral key | §3 | Plain key, no special meaning | passing → Batch 3 RB-026 |
| HIST-TOML-00M | Secrets absent from error output | §3 | Sanitized diagnostics (Vault CVE-2025-4166 class) | passing → Batch 3 RB-025 |
| HIST-TOML-00N | Table-merge round-trip | §2 | Merged structure reprints equivalently | passing → Batch 6 RB-055 (W011 + last-wins) |
| HIST-TOML-00P | Comments survive round-trip | §2 | Adjacent comments kept, data identical | passing → Batch 15 RB-146 |
| HIST-YAML-026 | Unsafe deserialization RCE (py) | §3 | N/A: no tags/constructors; reject `!` tags | passing → Batch 3 RB-023 |
| HIST-YAML-027 | Unsafe deserialization RCE (java) | §3 | N/A: no class system; reject | passing → Batch 3 RB-023 |
| HIST-YAML-028 | Unsafe deserialization RCE (js) | §3 | N/A: hermetic eval, no host calls | passing → Batch 3 RB-023 |
| HIST-YAML-029 | No safe/unsafe loader split | §3 | Single secure mode; no unsafe flag exists | passing → Batch 7 RB-063 (zero unsafe blocks scanned) |
| HIST-YAML-030 | Custom constructor abuse | §3 | No user-defined constructors invoked | passing → Batch 7 RB-062 |
| HIST-YAML-031 | `__proto__` parses as plain key | §3 | Same as HIST-TOML-00L | passing → Batch 3 RB-026 |
| HIST-YAML-036 | SSTI: `{{7*7}}` stays literal | §3 | Interpolation is `${…}`-only; braces literal | passing → Batch 3 RB-024 |
| HIST-YAML-037 | ReDoS via schema regex | §3 | Schema patterns bounded or engine-safe | passing → Batch 7 RB-064 (linear engine + T003) |
| HIST-YAML-040 | Error output sanitized | §3 | Same as HIST-TOML-00M | passing → Batch 3 RB-025 |
| HIST-YAML-041 | Include path sandbox | §3 | Literal/relative paths only, traversal rejected | passing → Batch 6 RB-056 (E015) |
| HIST-YAML-042 | Dup-key parser differential | §3 | Single deterministic policy everywhere | passing → Batch 6 RB-057 (parse ≡ parse_file) |
| HIST-YAML-001 | Multiline comma in array | §1 | Comma-in-string vs separator distinguished | passing → Batch 7 RB-067 |
| HIST-YAML-002 | Complex keys rejected | §1 | Only scalar keys; clear diagnostic | passing → Batch 7 RB-068 |
| HIST-YAML-005 | First-wins alternative | §1 | **Not adopted**: documents why last-wins won | passing → Batch 8 RB-071 (both syntaxes agree) |
| HIST-YAML-012 | Chomping behavior | §1 | N/A: no chomping indicators; triple-quote rules pinned | passing → Batch 7 RB-070 |
| HIST-YAML-017 | Anchor collision | §1 | N/A: no anchors; `let` redefinition policy pinned instead | passing → Batch 9 RB-081 (W011 shadow) |
| HIST-YAML-018 | Merge-key type error | §1 | N/A: no merge keys | passing (refusal pinned: no merge-key operator; merge() is E002 → Batch 10 RB-095) |
| HIST-YAML-023 | Self-reference via anchors | §1 | N/A: `let` self-reference cycle class instead | passing (covered: Batch 4 RB-036 self-ref S001) |
| HIST-TOML-009 | Ambiguous local time | §2 | N/A: no datetimes (importer strings) | passing → Batch 10 RB-098 (W008 string) |
| HIST-TOML-00A | Skipped local time | §2 | N/A: no datetimes (importer strings) | passing → Batch 10 RB-099 (W008 string) |
| HIST-TOML-00G | Large integer parse time | §2 | Linear-time digit handling | passing → Batch 9 RB-084 (1000 digits, E023) |
| HIST-TOML-00H | Long datetime parse time | §2 | N/A: no datetimes | passing → Batch 15 RB-147 (8KB datetime-lookalike parses as exact string) |
| HIST-TOML-00I | Max line length | §2 | Documented: bounded or explicitly unbounded | passing → Batch 9 RB-085 (1MB line terminates) |
| HIST-TOML-00J | Max document size | §2 | Documented input-size posture | passing → Batch 4 RB-035 |
| HIST-TOML-00O | Datetime round-trip | §2 | N/A: datetimes are strings | passing → Batch 15 RB-148 |
| HIST-YAML-032 | Merge taint | §3 | N/A: no merge operator | passing → Batch 10 RB-095 (merge() is E002) |
| HIST-YAML-033 | Deep-copy taint | §3 | Clone paths keep `Secret` kind | passing → Batch 8 RB-074 |
| HIST-YAML-034 | Spread taint | §3 | N/A: no spread operator | passing → Batch 10 RB-096 (`...` is E002) |
| HIST-YAML-035 | Dynamic-access taint | §3 | Indexing a secret path stays secret-typed or refused | passing → Batch 8 RB-073 (no index operator; elements redact) |
| HIST-YAML-038 | ReDoS in evaluation | §3 | No regex builtins; interpolation bounded | passing → Batch 8 RB-075 (100KB terminates) |
| HIST-YAML-039 | SSRF path traversal | §3 | String concat is literal; no fetch builtins | passing → Batch 9 RB-082 |
| HIST-YAML-043 | Boolean determinism | §3 | One boolean spelling, one value everywhere | passing → Batch 8 RB-076 |
| HIST-YAML-044 | URL backslash ambiguity | §3 | No URL parser in core; concat stays literal | passing → Batch 10 RB-097 |
| HIST-YAML-045 | Signature/duplicate-key split | §3 | Same as HIST-YAML-042 + sign.rs coverage | passing → Batch 8 RB-072 (sign/verify + tamper-fail) |

## §H — Catalogue families (`SOLID-Historical-Test-Catalogue-1000plus.md`)

| Family | IDs | Mining note | Status |
|---|---|---|---|
| SRC-TOML (95) | `SRC-TOML-0001`…`0095` | Upstream toml-test invalid fixtures → Active analogues (reject classes, escapes, tables) | complete: all 95 pinned → Batches 16–25; escape alphabet exactly the specified table (verbatim unknowns, no \u/surrogate decoding) |
| SRC-JSON (34) | `SRC-JSON-0001`…`0034` | JSONTestSuite edges → Active analogues (nesting, numbers, escapes, dup keys) | complete: all 34 pinned → Batches 25/26/27/28; depth budget E004 (100k brackets trip, 500-deep at bound 64); NUL-outside-string E005 |
| SRC-YAML (15) | `SRC-YAML-0001`…`0015` | yaml-test-suite edges → Active analogues | complete: all 15 pinned → Batches 29 (RB-279…288) + 30 (RB-289…293); chomping explicit per form; interp cycles are S001 |
| SRC-CVE (10) | `SRC-CVE-0001`…`0010` | CVE-derived payloads → budget/cycle/sanitization analogues | complete: all 10 pinned → Batches 30 (RB-294…298) + 31 (RB-299…303); unsafe-load impossible (E002 Error), bare interp E005, RFC8259 dup spanned |
| SRC-SPEC (10) | `SRC-SPEC-0001`…`0010` | Standards-spec edges → grammar conformance analogues | complete: all 10 pinned → Batches 31 (RB-304…308) + 32 (RB-309…313); escape alphabet matrix; quoted datetimes typed strings |
| Byte/token fixtures | `#` headings (~117) | Raw-byte edges (CR/NUL/TAB/controls); byte-exact inputs live beside tests | mining: SYN-0021-01/03/04 + NUL-only + empty pinned → Batch 32 RB-314…RB-318 (5 fixtures); earlier `#` headings covered in Batches 18–20/24/27–29 |

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
| Batch 5 | RB-041…RB-050 (HIST-YAML-008/010, HIST-TOML-003/006/007/00D/00E/00F, P0-014, HIST-JSON-003) | done: 9 green first-try; RB-049 corrected (H001 heals, doesn't fail) + RB-050 refusal-pinned (verbatim escapes per spec) |
| Batch 6 | RB-051…RB-060 | done: 9 green first-try; RB-051/052 drove the quoted-key E002 fix (silent drop eliminated) |
| Batch 7 | RB-061…RB-070 (HIST-YAML-001/002/012/016/029/030/037, DERIVED-0011/0017, P0-011) | done: 9 green first-try; RB-064/RB-065 corrections from shipped behavior |
| Batch 8 | RB-071…RB-080 | done: 9 green first-try; RB-079 drove CRLF normalization across all 4 string lexers |
| Batch 9 | RB-081…RB-090 | done: 9 green first-try; RB-085/RB-088 needed test-side arithmetic corrections |
| Batch 10 | RB-091…RB-100 | done: 9 green first-try; RB-100 forced a revert (statement-level skip is spec §4.1 design, not a bug) |
| Batch 11 | RB-101…RB-110 | done: all 10 green first-try; RB-101 drove the statement-initial E002 fix |
| Batch 12 | RB-111…RB-120 (P0-012, P1-ST-003/004/006/007/009/010/011/012/013) + dup-folds (P0-008/009/010/013/018, HIST-YAML-018/023, P1-ST-008) | done: all 10 green first-try, no code changes; RB-119 refusal-pinned (H002 stray-comma policy) |
| Batch 13 | RB-121…RB-130 (P1-TI-010, P1-ST-014/015, P1-SE-001/002/003/004/005/007/008) | done: all 10 green first-try, no code changes; secrets/units/normalization all shipped behavior pinned |
| Batch 14 | RB-131…RB-140 (P1-SE-011/012/014/015, P1-DS-001/002/003/004/005/009) | done: 9 green first-try; RB-138 needed a test-side oracle correction (model equality, not string order — order is DS-013's domain); RB-133 uses the real YAML/TOML exporters |
| Batch 15 | RB-141…RB-148 (P1-DS-010/011/012/013/014, HIST-TOML-00P/00H/00O) | done: all 8 green first-try; RB-143 refusal-pinned (fmt re-emits secret() calls per spec/secrets.md:48-49) |
| Batch 16 | RB-149…RB-158 (HIST-YAML-006, SRC-TOML-0001…0009) | done: all 10 green first-try, no code changes; catalogue mining started (oldest-first); refusals vs upstream-reject pinned (H002/H003/W011 policies) |
| Batch 17 | RB-159…RB-168 (SRC-TOML-0010…0019) | done: all 10 green first-try, no code changes; bool exact-token + prefix-match refusal pinned (W001+null); catalogue typo noted (0018 prints TRUE, title/origin name FALSE) |
| Batch 18 | RB-169…RB-178 (SRC-TOML-0020…0029) | done: all 10 green first-try, no code changes; byte-fixture batch (8 committed fixtures under tests/research/); refusals per grammar §1.2/§1.4/§2.1 + RB-092 oracle |
| Batch 19 | RB-179…RB-188 (SRC-TOML-0030…0039) | done: all 10 green first-try, no code changes; 0x1F fixture committed; no-date-type policy pinned (arithmetic values or E002, quoted stays string) |
| Batch 20 | RB-189…RB-198 (SRC-TOML-0040…0049) | done: all 10 green first-try, no code changes; 6 encoding fixtures committed; invalid UTF-8 → E017, misplaced BOM/U+3000 → E005 |
| Batch 21 | RB-199…RB-208 (SRC-TOML-0050…0059) | done: all 10 green first-try, no code changes; UTF-16 fixture committed; float-grammar edges pinned (E002/E005/W001, decimal leading zeros, int-keeping dots) |
| Batch 22 | RB-209…RB-218 (SRC-TOML-0060…0069) | done: 8 green first-try; RB-211/RB-215 needed test-side span corrections (W011 span marks winning value inline, redefined key in block form); RB-218 control corrected (no unary + in grammar, only -); inline-table policy fully pinned |
| Batch 23 | RB-219…RB-228 (SRC-TOML-0070…0079) | done: all 10 green first-try, no code changes; base prefixes absent (E002); leading `_` is identifier-shaped (W001+null); quoted dotted keys never collide (E002) |
| Batch 24 | RB-229…RB-238 (SRC-TOML-0080…0089) | done: all 10 green first-try, no code changes; 2 multiline-key fixtures committed; key grammar strict (E002 everywhere); byte escapes verbatim (no invalid Unicode possible) |
| Batch 25 | RB-239…RB-248 (SRC-TOML-0090…0095, SRC-JSON-0001…0004) | done: all 10 green first-try, no code changes; SRC-TOML family complete (95/95); JSON mining open (0001…0004); escape alphabet exactly specified; H002 Info severity pinned |
| Batch 26 | RB-249…RB-258 (SRC-JSON-0005…0014) | done: all 10 green individually; one transient whole-file single failure (unidentified, 3/4 file runs 258/258 green — logged as residual risk); JSON number edges pinned (decimal zeros, no exponents, first-value floats) |
| Batch 27 | RB-259…RB-268 (SRC-JSON-0015…0024) | done: all 10 green first-try, no code changes; 2 string fixtures committed; object grammar + content-exact strings pinned |
| Batch 28 | RB-269…RB-278 (SRC-JSON-0025…0034) | done: all 10 green first-try, no code changes; SRC-JSON family complete (34/34); NUL-outside-string fixture committed; CRITICAL depth-budget cases pin E004 |
| Batch 29 | RB-279…RB-288 (SRC-YAML-0001…0010) | done: all 10 green first-try, no code changes; TAB-indent fixture committed; YAML mining open (anchors absent, Norway refused, merge via @patch) |
| Batch 30 | RB-289…RB-298 (SRC-YAML-0011…0015, SRC-CVE-0001…0005) | done: all 10 green first-try, no code changes; SRC-YAML family complete (15/15); CVE mining open with bounded generators (E004, exact fan-out, tags never execute) |
| Batch 31 | RB-299…RB-308 (SRC-CVE-0006…0010, SRC-SPEC-0001…0005) | done: all 10 green individually; one transient whole-file single failure (2nd occurrence, 2/3 file runs 308/308 green — residual risk updated); SRC-CVE complete (10/10); SPEC mining open |
| Batch 32 | RB-309…RB-318 (SRC-SPEC-0006…0010, byte-fixture mining 5) | done: 9 green first-try; RB-309 needed a test-side escape fix (Rust newlines vs Active escapes); SRC-SPEC complete (10/10); 5 byte fixtures committed |
| Batch 33 | RB-319…RB-328 (SYN-0001-01…09, SYN-0002-01) | done: all 10 green first-try, no code changes; SYN mining open (variant-specific assertions: no-null, no-merge, comma≠value, EOF span, twin convergence) |
| Batch 34 | RB-329…RB-338 (SYN-0002-02…09, SYN-0003-01…02) | done: all 10 green first-try, no code changes; 02-lineage spans/severities/types indexed; redefinition announcement = anti-silence; patch paths type-check first |
| Batch 35 | RB-339…RB-348 (SYN-0003-03…09, SYN-0004-01…03) | done: all 10 green first-try, no code changes; naive patch/remove/rename/index forms all E019; explicit call forms pinned (patch/remove/rename/nested, ordered plans) |
| Batch 36 | RB-349…RB-358 (SYN-0004-04…09, SYN-0005-01…04) | done: all 10 green first-try, no code changes; 04-lineage E019 spans at 2:2 + sibling-aware controls; 05-lineage severities/spans (H002 Info, H003 Warning, comma spans) |
| Batch 37 | RB-359…RB-368 (SYN-0005-05…09, SYN-0006-01…05) | done: all 10 green first-try, no code changes; determinism indexed on accepts; heal/insert severity triples; lone/trailing comma spans per key width |
| Batch 38 | RB-369…RB-378 (SYN-0006-06…09, SYN-0007-01…06) | done: all 10 green first-try, no code changes; 06-lineage completed (determinism, nested types, hetero values, EOF triple); 07-lineage opened (heal/insert triples, comma spans, control determinism) |
| Batch 39 | RB-379…RB-388 (SYN-0007-07…09, SYN-0008-01…07) | done: all 10 green first-try, no code changes; 07-lineage completed (lengths, joint hetero oracle, EOF triple); 08-lineage opened (heal/insert triples, comma spans, typed determinism) |
| Batch 40 | RB-389…RB-398 (SYN-0008-08…09, SYN-0009-01…08) | done: all 10 green first-try, no code changes; 08-lineage completed (hetero export, EOF triple); 09-lineage opened (heal/insert triples, comma spans, typed determinism) |
| Batch 41 | RB-399…RB-408 (SYN-0009-09, SYN-0010-01…09) | done: all 10 green first-try, no code changes; 09-lineage completed (EOF triple); 10-lineage opened (heal/insert triples, comma spans, typed determinism) |
| Batch 42 | RB-409…RB-418 (SYN-0011-01…09, SYN-0012-01) | done: all 10 green first-try, no code changes; 11-lineage completed (heal/insert triples, comma spans, typed determinism); 12-lineage opened (no-null scan) |
| Batch 43 | RB-419…RB-428 (SYN-0012-02…09, SYN-0013-01…02) | done: all 10 green first-try, no code changes; 12-lineage completed (insert/lone/trailing triples, typed determinism, hetero joint, EOF triple); 13-lineage opened (heal/insert triples) |
| Batch 44 | RB-429…RB-438 (SYN-0013-03…09, SYN-0014-01…03) | done: all 10 green first-try, no code changes; 13-lineage completed (lone/trailing triples, typed determinism, hetero joint, EOF triple); 14-lineage opened (exactly-one-warning, E019 severities) |
| Batch 45 | RB-439…RB-448 (SYN-0014-04…09, SYN-0015-01…04) | done: all 10 green first-try, no code changes; 14-lineage completed (hybrid refusal, patch/remove/rename/order/nesting determinism); 15-lineage opened (no-null, announced-separator, lone/trailing comma spans) |
| Batch 46 | RB-449…RB-458 (SYN-0015-05…09, SYN-0016-01…05) | done: all 10 green first-try, no code changes; 15-lineage completed (clean/empty/nested/hetero accepts, unterminated EOF 2:1); 16-lineage opened (bare yes/no/on/off W001+null never bool, reserved true stays bool) |
| Batch 47 | RB-459…RB-468 (SYN-0016-06…09, SYN-0017-01…06) | done: all 10 green first-try, no code changes; 16-lineage completed (FALSE W001+null, null reserved vs NULL W001, quoted "no" exact, 1.0 stays float); 17-lineage opened (twin convergence yes/no/on/off/true/FALSE) |
| Batch 48 | RB-469…RB-478 (SYN-0017-07…09, SYN-0018-01…07) | done: all 10 green first-try, no code changes; 17-lineage completed (null/"no"/1.0 twins agree); 18-lineage opened (three-lineage convergence yes/no/on/off/true/FALSE/null) |
| Batch 49 | RB-479…RB-488 (SYN-0018-08…09, SYN-0019-01…08) | done: all 10 green first-try, no code changes; 18-lineage completed (quoting/number twins agree); 19-lineage opened (four-lineage convergence yes/no/on/off/true/FALSE/null/quoting) |
| Batch 50 | RB-489…RB-498 (SYN-0019-09, SYN-0020-01…09) | done: all 10 green first-try, no code changes; 19-lineage completed (four-lineage number agreement); 20-lineage opened (five-lineage convergence yes/no/on/off/true/FALSE/null/quoting/numbers) |
| Batch 51 | RB-499…RB-508 (SYN-0021-02,04…09, SYN-0022-01…03) | done: all 10 green first-try, no code changes; 21-lineage mined (E017 key/trunc4, verbatim \u refusal, TAB-in-string, café/𝄞 exact, leading-escape value); 22-lineage opened (NUL/E017/BOM twins converge) |
| Batch 52 | RB-509…RB-518 (SYN-0022-04…09, SYN-0023-01…04) | done: all 10 green first-try, no code changes; 22-lineage completed (verbatim/TAB/café/clef/trunc3/mid-escape agree); 23-lineage opened (NUL/E017/BOM-1:16 triplets + verbatim \u) |
| Batch 53 | RB-519…RB-528 (SYN-0023-05…09, SYN-0024-01…05) | done: all 10 green first-try, no code changes; 23-lineage completed (TAB/café/clef/trunc2/mid-escape agree); 24-lineage opened (NUL-2:1, UTF-8 quad, BOM-line2, verbatim \u, TAB quad) |
| Batch 54 | RB-529…RB-538 (SYN-0024-06…09, SYN-0025-01…06) | done: all 10 green first-try, no code changes; 24-lineage completed (café/clef/truncE2-array/mid-escape agree); 25-lineage opened (NUL-in-string/comment content vs outside-E005, UTF-8 quint, BOM-after-header 1:4, verbatim \u, TAB, café) |
| Batch 55 | RB-539…RB-548 (SYN-0025-07…09, SYN-0026-01…07) | done: all 10 green first-try, no code changes; 25-lineage completed (clef/trunc4b3/mid-escape agree); 26-lineage opened (NUL-in-header 1:3, UTF-8 sextet, BOM-after-indent 1:3+E010, verbatim \u, TAB, café, clef) |
| Batch 56 | RB-549…RB-558 (SYN-0026-08…09, SYN-0027-01…08) | done: all 10 green first-try, no code changes; 26-lineage completed (truncE282-array 6-twin loop, mid-escape sextet); 27-lineage opened (NUL-split-key 1:6, UTF-8 septet, BOM-in-comment inert, verbatim \u, TAB, café, clef, trunc-in-comment E017) |
| Batch 57 | RB-559…RB-568 (SYN-0027-09, SYN-0028-01…09) | done: all 10 green first-try, no code changes; 27-lineage completed (mid-escape septet); 28-lineage opened (NUL-in-array 1:6, UTF-8 octet, BOM-split-key 1:2, verbatim \u, TAB, café, clef, lone-F0 E017, mid-escape octet) |
| Batch 58 | RB-569…RB-578 (SYN-0029-01…09, SYN-0030-01) | done: all 10 green first-try, no code changes; 29-lineage completed (NUL-value 1:5, UTF-8 nonet, BOM-at-EOF 2:1, verbatim \u, TAB, café, clef, trunc-pair-comment, mid-escape nonet); 30-lineage opened (NUL-after-indent 1:3+E010) |
| Batch 59 | RB-579…RB-588 (SYN-0030-02…09, SYN-0031-01…02) | done: all 10 green first-try, no code changes; 30-lineage completed (UTF-8 decet, BOM-in-string data, verbatim \u, TAB, café, clef, trunc-in-key, mid-escape decet); 31-day-zero opened (Feb-30 is arithmetic 1992, offset form degrades to 1993+T12 pair — no-date refusals pinned) |
| Batch 60 | RB-589…RB-598 (SYN-0031-03…09, SYN-0032-01…03) | done: all 10 green first-try, no code changes; 31-lineage completed (Z-forms E002 1:32/1:45/1:35, quoted never validated, offset degrades clean, leading-zero decimal, quoted preserved, bad-minutes E002); 32-feb29 opened (Feb-30/offset/Z twins converge) |
| Batch 61 | RB-599…RB-608 (SYN-0032-04…09, SYN-0033-01…04) | done: all 10 green first-try, no code changes; 32-lineage completed (quoted/fractional/offset/leading-zero/datetime-preserved/bad-minutes twins converge); 33-feb30 opened (Feb-30 third twin, offset twins collide loud W011 2:26, Z/quoted thirds converge) |
| Batch 62 | RB-609…RB-618 (SYN-0033-05…09, SYN-0034-01…05) | done: all 10 green first-try, no code changes; 33-lineage completed (fractional/offset/leading-zero/datetime-preserved/bad-minutes thirds converge); 34-hour-over opened (Feb-30 fourth twin, offset fourth degrades, Z/quoted/fractional fourths converge) |
| Batch 63 | RB-619…RB-628 (SYN-0034-06…09, SYN-0035-01…06) | done: all 10 green first-try, no code changes; 34-lineage completed (offset/leading-zero/datetime-preserved/bad-minutes fourths converge); 35-leading-zero-date opened (Feb-30 fifth twin, offset fifth degrades, Z/quoted/fractional/offset fifths converge) |
| Batch 64 | RB-629…RB-638 (SYN-0035-07…09, SYN-0036-01…07) | done: all 10 green first-try, no code changes; 35-lineage completed (leading-zero/datetime-preserved/bad-minutes fifths converge); 36-no-secs opened (Feb-30 sixth twin, offset sixth degrades, Z/quoted/fractional/offset/leading-zero sixths converge) |
| Batch 65 | RB-639…RB-648 (SYN-0036-08…09, SYN-0037-01…08) | done: all 10 green first-try, no code changes; 36-lineage completed (quoted-datetime/bad-minutes sixths converge); 37-no-t opened (Feb-30 seventh twin, offset seventh degrades, Z/quoted/fractional/offset/leading-zero/datetime sevenths converge) |
| Batch 66 | RB-649…RB-658 (SYN-0037-09, SYN-0038-01…09) | done: all 10 green first-try, no code changes; 37-lineage completed (bad-minutes seventh rejects); 38-offset-overflow-hour opened (Feb-30 eighth twin, offset eighth degrades, Z/quoted/fractional/offset/leading-zero/datetime/bad-minutes eighths converge) |
| Batch 67 | RB-659…RB-668 (SYN-0039-01…09, SYN-0040-01) | done: all 10 green first-try, no code changes; 39-offset-overflow-minute opened (Feb-30 ninth twin, offset ninth degrades, Z/quoted/fractional/offset/leading-zero/datetime/bad-minutes ninths converge); 40-second-over opened (Feb-30 tenth twin) |
| Batch 68 | RB-669…RB-678 (SYN-0040-02…09, SYN-0041-01…02) | done: all 10 green first-try, no code changes; 40-lineage completed (offset tenth degrades, Z/quoted/fractional/offset/leading-zero/datetime/bad-minutes tenths converge); 41-second-trailing-dot opened (Feb-30 eleventh twin, offset eleventh degrades) |
| Batch 69 | RB-679…RB-688 (SYN-0041-03…09, SYN-0042-01…03) | done: all 10 green first-try, no code changes; 41-lineage completed (Z/quoted/fractional/offset/leading-zero/datetime/bad-minutes elevenths converge); 42-trailing-x opened (Feb-30 twelfth twin, offset twelfth degrades, Z twelfth rejects) |
| Batch 70 | RB-689…RB-698 (SYN-0042-04…09, SYN-0043-01…04) | done: all 10 green first-try, no code changes; 42-lineage completed (quoted/fractional/offset/leading-zero/datetime/bad-minutes twelfths converge); 43-y10k-date opened (Feb-30 thirteenth twin, offset thirteenth degrades, Z/quoted thirteenths converge) |
| Batch 71 | RB-699…RB-708 (SYN-0043-05…09, SYN-0044-01…05) | done: all 10 green first-try, no code changes; 43-lineage completed (fractional/offset/leading-zero/datetime-preserved/bad-minutes thirteenths converge); 44-bad-codepoint opened (NUL-in-number 1:7, overlong-NUL E017, double-BOM 1:1, verbatim \u, TAB-in-header inert) |
| Batch 72 | RB-709…RB-718 (SYN-0044-06…09, SYN-0045-01…06) | done: all 10 green first-try, no code changes; 44-lineage completed (café/clef/trunc-in-header/mid-escape agree); 45-bad-utf8-at-end opened (NUL-in-quoted-key E002, UTF-8-in-comment E017, BOM-after-eq 1:5, verbatim \u, TAB, café) |
| Batch 73 | RB-719…RB-728 (SYN-0045-07…09, SYN-0046-01…07) | done: all 10 green first-try, no code changes; 45-lineage completed (clef/surrogate-E017/mid-escape agree); 46-bad-utf8-in-array opened (NUL-post-comma 1:10, 0xFF E017, BOM-in-array 1:6, verbatim \u, TAB, café, clef) |
| Batch 74 | RB-729…RB-738 (SYN-0046-08…09, SYN-0047-01…08) | done: all 10 green first-try, no code changes; 46-lineage completed (trunc-E2-array/mid-escape agree); 47-bad-utf8-in-string opened (NUL-after-value 1:6, lone-continuation E017, BOM-open-bracket 1:2, verbatim \u, TAB, café, clef, trunc-F0-9D-string) |
| Batch 75 | RB-739…RB-748 (SYN-0047-09, SYN-0048-01…09) | done: all 10 green first-try, no code changes; 47-lineage completed (mid-escape fourteenth agrees); 48-bom-not-at-start opened (NUL-in-braces 1:6, overlong-in-comment E017, BOM-post-comma 1:10, verbatim \u, TAB, café, clef, overlong-3byte E017, mid-escape fifteenth) |
| Batch 76 | RB-749…RB-758 (SYN-0049-01…09, SYN-0050-01) | done: all 10 green first-try, no code changes; 49-ideographic-space opened (NUL-mid-line 1:6, UTF-8-in-header E017, BOM-in-number 1:7, verbatim \u, TAB, café, clef, trunc-pair-key, mid-escape); 50-utf16-bom opened (UTF-16LE is E017, never mojibake) |
| Batch 77 | RB-759…RB-768 (SYN-0050-02…09, SYN-0051-01…02) | done: all 10 green first-try, no code changes; 50-lineage completed (UTF-8-in-number E017, BOM-after-colon 1:4, verbatim \u, TAB, café, clef, trunc-F0-9D-84-array, mid-escape); 51-arabic-zero opened (`01` is decimal 1 refusal-pinned, `1.` is int 1 refusal-pinned) |
| Batch 78 | RB-769…RB-778 (SYN-0051-03…09, SYN-0052-01…03) | done: 9 green first-try; RB-772 red on a test-side arithmetic typo (1_000 is 1000, not 1000000 — code was right per RB-107), oracle corrected to the research expectation, re-run green + file green; 51-lineage completed (bare/signed exponents, hex, underscore, giant-exponent E002s, quoted digits, canonical zero); 52-double-dot opened (`01`/1-decimal and `1.`/int twins converge) |
| Batch 79 | RB-779…RB-788 (SYN-0052-04…09, SYN-0053-01…04) | done: all 10 green first-try, no code changes; 52-lineage completed (signed-exponent/hex/underscore-1000/giant-exponent/quoted-digits/canonical-zero twins converge); 53-exp-dot opened (`01`-decimal, `1.`-int, bare/signed-exponent E002 thirds converge) |
| Batch 80 | RB-789…RB-798 (SYN-0053-05…09, SYN-0054-01…05) | done: all 10 green first-try, no code changes; 53-lineage completed (hex/underscore-1000/giant-exponent/quoted-digits/canonical-zero thirds converge); 54-exp-double-e opened (`01`-decimal, `1.`-int, bare/signed-exponent E002, hex fourths converge) |
| Batch 81 | RB-799…RB-808 (SYN-0054-06…09, SYN-0055-01…06) | done: all 10 green first-try, no code changes; 54-lineage completed (underscore-1000/giant-exponent/quoted-digits/canonical-zero fourths converge); 55-leading-dot opened (`01`-decimal, `1.`-int, bare/signed-exponent E002, hex, underscore-1000 fifths converge) |
| Batch 82 | RB-809…RB-818 (SYN-0055-07…09, SYN-0056-01…07) | done: all 10 green first-try, no code changes; 55-lineage completed (giant-exponent/quoted-digits/canonical-zero fifths converge); 56-leading-zero opened (`01`-decimal, `1.`-int, bare/signed-exponent E002, hex, underscore-1000, giant-exponent sixths converge) |
| Batch 83 | RB-819…RB-828 (SYN-0056-08…09, SYN-0057-01…08) | done: all 10 green first-try, no code changes; 56-lineage completed (quoted-digits/canonical-zero sixths converge); 57-nan-capital opened (`01`-decimal, `1.`-int, bare/signed-exponent E002, hex, underscore-1000, giant-exponent, quoted-digits sevenths converge) |
| Batch 84 | RB-829…RB-838 (SYN-0057-09, SYN-0058-01…09) | done: all 10 green first-try, no code changes; 57-lineage completed (canonical-zero seventh converges); 58-trailing-dot opened (`01`-decimal, `1.`-int, bare/signed-exponent E002, hex, underscore-1000, giant-exponent, quoted-digits, canonical-zero eighths converge) |
| Batch 85 | RB-839…RB-848 (SYN-0059-01…09, SYN-0060-01) | done: all 10 green first-try, no code changes; 59-trailing-exp opened (`01`-decimal, `1.`-int, bare/signed-exponent E002, hex, underscore-1000, giant-exponent, quoted-digits, canonical-zero ninths converge); 60-trailing-us opened (`01` tenth twin converges) |
| Batch 86 | RB-849…RB-858 (SYN-0060-02…09, SYN-0061-01…02) | done: all 10 green first-try, no code changes; 60-lineage completed (`1.`-int, bare/signed-exponent E002, hex, underscore-1000, giant-exponent, quoted-digits, canonical-zero tenths converge); 61-us-after-dot opened (`01`-decimal, `1.`-int elevenths converge) |
| Batch 87 | RB-859…RB-868 (SYN-0061-03…09, SYN-0062-01…03) | done: all 10 green first-try, no code changes; 61-lineage completed (bare/signed-exponent E002, hex, underscore-1000, giant-exponent, quoted-digits, canonical-zero elevenths converge); 62-duplicate-key opened (inline-dup W011 1:33 + nested-dup W011 1:44 refusal-pinned to last-wins-announce; quoted-never-merges E001 1:18) |
| Batch 88 | RB-869…RB-878 (SYN-0062-04…09, SYN-0063-01…04) | done: all 10 green first-try, no code changes; 62-lineage completed (distinct-keys silent, unicode-quoted E001, distinct-tables accept, naive-patch E019 vs explicit, double-remove W004, dotted-alias E001); 63-linebreak opened (multi-line H002 2:1, H003 2:1, lone 2:1, trailing 1:18) |
| Batch 89 | RB-879…RB-888 (SYN-0063-05…09, SYN-0064-01…05) | done: all 10 green first-try, no code changes; 63-lineage completed (multi-line clean/empty/nested/hetero silent, unterminated EOF 2:1); 64-no-close opened (doubled/missing/lone/trailing comma rhythm + clean-array control) |
| Batch 90 | RB-889…RB-898 (SYN-0064-06…09, SYN-0065-01…06) | done: all 10 green first-try, no code changes; 64-lineage completed (multi-line empty/nested/hetero silent, unterminated EOF 2:1); 65-no-comma opened (doubled/missing/lone/trailing comma rhythm + clean/empty-array controls) |
| Batch 91 | RB-899…RB-908 (SYN-0065-07…09, SYN-0066-01…07) | done: all 10 green first-try, no code changes; 65-lineage completed (multi-line nested/hetero silent, unterminated EOF 2:1); 66-overwrite opened (inline-dup W011 1:33 + nested-dup W011 1:44 refusal-pinned; quoted-never-merges E001; distinct-keys/tables accept; naive-patch E019 vs explicit) |
| Batch 92 | RB-909…RB-918 (SYN-0066-08…09, SYN-0067-01…08) | done: all 10 green first-try, no code changes; 66-lineage completed (naive-repeat fails first + explicit-remove control; dotted-alias E001 twin); 67-trailing-comma opened (doubled/missing/lone/trailing comma rhythm + clean/empty/nested/hetero accepts) |
| Batch 93 | RB-919…RB-928 (SYN-0067-09, SYN-0068-01…09) | done: all 10 green first-try, no code changes; 67-lineage completed (unterminated EOF 2:1 twin); 68-capital-hex opened (`01`-decimal, `1.`-int, bare/signed-exponent E002, hex, underscore-1000, giant-exponent, quoted-digits, canonical-zero twelfths converge) |
| Batch 94 | RB-929…RB-938 (SYN-0069-01…09, SYN-0070-01) | done: all 10 green first-try, no code changes; 69-double-sign-plus opened (`01`-decimal, `1.`-int, bare/signed-exponent E002, hex, underscore-1000, giant-exponent, quoted-digits, canonical-zero thirteenths converge); 70-incomplete-hex opened (`01` fourteenth twin converges) |
| Batch 95 | RB-939…RB-948 (SYN-0070-02…09, SYN-0071-01…02) | done: all 10 green first-try, no code changes; 70-lineage completed (`1.`-int, bare/signed-exponent E002, hex, underscore-1000, giant-exponent, quoted-digits, canonical-zero fourteenths converge); 71-invalid-bin opened (`01`-decimal, `1.`-int fifteenths converge) |
| Batch 96 | RB-949…RB-958 (SYN-0071-03…09, SYN-0072-01…03) | done: all 10 green first-try, no code changes; 71-lineage completed (bare/signed-exponent E002, hex, underscore-1000, giant-exponent, quoted-digits, canonical-zero fifteenths converge); 72-leading-us opened (`01`-decimal, `1.`-int, bare-exponent sixteenths converge) |
| Batch 97 | RB-959…RB-968 (SYN-0072-04…09, SYN-0073-01…04) | done: all 10 green first-try, no code changes; 72-lineage completed (signed-exponent/hex/underscore-1000/giant-exponent/quoted-digits/canonical-zero sixteenths converge); 73-leading-zero-01 opened (`01`-decimal, `1.`-int, bare/signed-exponent seventeenths converge) |
| Batch 98 | RB-969…RB-978 (SYN-0073-05…09, SYN-0074-01…05) | done: all 10 green first-try, no code changes; 73-lineage completed (hex/underscore-1000/giant-exponent/quoted-digits/canonical-zero seventeenths converge); 74-negative-hex opened (`01`-decimal, `1.`-int, bare/signed-exponent E002, hex eighteenths converge) |
| Batch 99 | RB-979…RB-988 (SYN-0074-06…09, SYN-0075-01…06) | done: all 10 green first-try, no code changes; 74-lineage completed (underscore-1000/giant-exponent/quoted-digits/canonical-zero eighteenths converge); 75-trailing-us opened (`01`-decimal, `1.`-int, bare/signed-exponent E002, hex, underscore-1000 nineteenths converge) |
| Batch 100 | RB-989…RB-998 (SYN-0075-07…09, SYN-0076-01…07) | done: all 10 green first-try, no code changes; 75-lineage completed (giant-exponent/quoted-digits/canonical-zero nineteenths converge); 76-us-after-bin opened (`01`-decimal, `1.`-int, bare/signed-exponent E002, hex, underscore-1000, giant-exponent twentieths converge) |
| Batch 101 | RB-999…RB-1008 (SYN-0076-08…09, SYN-0077-01…08) | done: all 10 green first-try, no code changes; 76-lineage completed (quoted-digits/canonical-zero twentieths converge); 77-after-array opened (inline-dup W011 1:33 + nested-dup W011 1:44 refusal-pinned; quoted-never-merges E001; distinct-keys/tables accept; naive-patch/naive-double-remove E019 vs explicit) |
| Batch 102 | RB-1009…RB-1018 (SYN-0077-09, SYN-0078-01…09) | done: all 10 green first-try, no code changes; 77-lineage completed (dotted-alias E001 closer); 78-bare-invalid-char opened+completed (inline-dup W011 1:33 + nested-dup W011 1:44 refusal-pinned; quoted-never-merges E001; distinct-keys/tables accept; naive-patch/naive-double-remove E019 vs explicit) |
| Batch 103 | RB-1019…RB-1028 (SYN-0079-01…09, SYN-0080-01) | done: all 10 green first-try, no code changes; 79-bare-invalid-char2 opened+completed (inline-dup W011 1:33 + nested-dup W011 1:44 refusal-pinned; quoted-never-merges E001; distinct-keys/tables accept; naive-patch/naive-double-remove E019 vs explicit; dotted-alias E001 closer); 80-lineage opened (inline-dup W011 1:33) |
| Batch 104 | RB-1029…RB-1038 (SYN-0080-02…09, SYN-0081-01…02) | done: all 10 green first-try, no code changes; 80-lineage completed (nested-dup W011 1:44 refusal-pinned; quoted-never-merges E001; distinct-keys/tables accept; naive-patch/naive-double-remove E019 vs explicit; dotted-alias E001 closer); 81-lineage opened (inline-dup W011 1:33 + nested-dup W011 1:44) |
| Batch 105 | RB-1039…RB-1048 (SYN-0081-03…09, SYN-0082-01…03) | done: all 10 green first-try, no code changes; 81-lineage completed (quoted-never-merges E001; distinct-keys/tables accept; naive-patch/naive-double-remove E019 vs explicit; dotted-alias E001 closer); 82-lineage opened (inline-dup W011 1:33 + nested-dup W011 1:44 + quoted-never-merges E001) |
| Batch 106 | RB-1049…RB-1058 (SYN-0082-04…09, SYN-0083-01…04) | done: all 10 green first-try, no code changes; 82-lineage completed (distinct-keys silent; NFC/NFD pair rejects pre-normalization E001; distinct-tables accept; naive-patch/naive-double-remove E019 vs explicit; dotted-alias E001 closer); 83-lineage opened (inline-dup W011 1:33 + nested-dup W011 1:44 + quoted-never-merges E001 + distinct-keys silent) |
| Batch 107 | RB-1059…RB-1068 (SYN-0083-05…09, SYN-0084-01…05) | done: all 10 green first-try, no code changes; 83-lineage completed (NFC/NFD pair rejects pre-normalization E001; distinct-tables accept; naive-patch/naive-double-remove E019 vs explicit; dotted-alias E001 closer); 84-lineage opened (inline-dup W011 1:33 + nested-dup W011 1:44 + quoted-never-merges E001 + distinct-keys silent + unicode E001) |
| Batch 108 | RB-1069…RB-1078 (SYN-0084-06…09, SYN-0085-01…06) | done: all 10 green first-try, no code changes; 84-lineage completed (distinct-tables accept; naive-patch/naive-double-remove E019 vs explicit; dotted-alias E001 closer); 85-lineage opened (inline-dup W011 1:33 + nested-dup W011 1:44 + quoted-never-merges E001 + distinct-keys silent + unicode E001 + distinct-tables accept) |
| Batch 109 | RB-1079…RB-1088 (SYN-0085-07…09, SYN-0086-01…07) | done: all 10 green first-try, no code changes; 85-lineage completed (naive-patch/naive-double-remove E019 vs explicit; dotted-alias E001 closer); 86-lineage opened (inline-dup W011 1:33 + nested-dup W011 1:44 + quoted-never-merges E001 + distinct-keys silent + unicode E001 + distinct-tables accept + naive-patch E019) |
| Batch 110 | RB-1089…RB-1098 (SYN-0086-08…09, SYN-0087-01…08) | done: all 10 green first-try, no code changes; 86-lineage completed (naive-double-remove E019 vs explicit; dotted-alias E001 closer); 87-lineage opened (inline-dup W011 1:33 + nested-dup W011 1:44 + quoted-never-merges E001 + distinct-keys silent + unicode E001 + distinct-tables accept + naive-patch/double-remove E019) |
| Batch 111 | RB-1099…RB-1108 (SYN-0087-09, SYN-0088-01…09) | done: all 10 green first-try, no code changes; 87-lineage completed (dotted-alias E001 closer); 88-lineage opened+completed (inline-dup W011 1:33 + nested-dup W011 1:44 refusal-pinned; quoted-never-merges E001; distinct-keys/tables accept; naive-patch/naive-double-remove E019 vs explicit; dotted-alias E001 closer) |
| Batch 112 | RB-1109…RB-1118 (SYN-0089-01…09, SYN-0090-01) | done: 9 green first-try; RB-1117 red on a test-side oracle error (unresolved interpolation renders as text "null" with W001 at 1:19, not JSON null — code untouched, oracle corrected to probe-observed behavior); 89-bad-byte-escape opened (dangling H001 1:16 refusal-pinned; \uD800/\u0061/\u0000/\x41 verbatim refusal-pinned, never decoded; \n decodes; literal newline spans; emoji NFD exact; escaped NUL never materializes, raw NUL E005); 90-lineage opened (dangling twin) |
| Batch 113 | RB-1119…RB-1128 (SYN-0090-02…09, SYN-0091-01…02) | done: all 10 green first-try, no code changes; 90-bad-escape-01 twins converge with 89-lineage (verbatim \u/\x refusal-pinned; \n decodes; newline spans; emoji NFD exact; interp text-"null" + W001); 91-bad-hex-esc opened (dangling H001 twin + surrogate verbatim) |
| Batch 114 | RB-1129…RB-1138 (SYN-0091-03…09, SYN-0092-01…03) | done: all 10 green first-try, no code changes; 91-bad-hex-esc continued (scalar verbatim; \n decodes; newline spans; \x41 verbatim in hex-named family; emoji NFD exact; NUL never materializes; interp text-"null" + W001); 92-bad-uni-esc opened (dangling H001 twin + surrogate/scalar verbatim) |
| Batch 115 | RB-1139…RB-1148 (SYN-0092-04…09, SYN-0093-01…04) | done: all 10 green first-try, no code changes; 92-bad-uni-esc continued (\n decodes; newline spans; \x41 verbatim; emoji NFD exact; NUL never materializes; interp text-"null" + W001); 93-bad-multiline opened (dangling H001 twin + surrogate/scalar verbatim + \n decodes) |
| Batch 116 | RB-1149…RB-1158 (SYN-0093-05…09, SYN-0094-01…05) | done: all 10 green first-try, no code changes; 93-bad-multiline continued (newline spans; \x41 verbatim; emoji NFD exact; NUL never materializes; interp text-"null" + W001); 94-bad-slash-escape opened (dangling H001 twin + surrogate/scalar verbatim + \n decodes + newline spans) |
| Batch 117 | RB-1159…RB-1168 (SYN-0094-06…09, SYN-0095-01…06) | done: all 10 green first-try, no code changes; 94-bad-slash-escape continued (\x41 verbatim; emoji NFD exact; NUL never materializes; interp text-"null" + W001); 95-bad-concat opened (dangling H001 twin + surrogate/scalar verbatim + \n decodes + newline spans + \x41 verbatim) |
| Batch 118 | RB-1169…RB-1178 (SYN-0095-07…09, SYN-0096-01…07) | done: all 10 green first-try, no code changes; 95-bad-concat tail (emoji NFD exact; NUL never materializes; interp text-"null" + W001); 96-JSON-comma opened (doubled H002 1:19 + missing H003 1:19 refusal-pinned; lonely H002 1:17; trailing H002 1:18 explicitly allowed; clean/empty/nested silent) |
| Batch 119 | RB-1179…RB-1188 (SYN-0096-08…09, SYN-0097-01…08) | done: all 10 green first-try, no code changes; 5 byte fixtures committed; 96-JSON-comma completed (hetero accepts; unterminated E001 EOF 2:1); 97-invalid-utf8-array opened (NUL E005 1:1; overlong E017 spanless; BOM E005 1:5; \u0001 verbatim; TAB E006 1:1; cafe/clef exact; truncated-E2 E017 spanless) |
| Batch 120 | RB-1189…RB-1198 (SYN-0097-09, SYN-0098-01…09) | done: all 10 green first-try, no code changes; 97-lineage completed (TAB escape decodes); 98-colon-instead-of-comma opened+completed (doubled H002 1:19 + missing H003 1:19 refusal-pinned; lonely H002 1:17; trailing H002 1:18 allowed; clean/empty/nested/hetero silent; unterminated E001 EOF 2:1) |
| Batch 121 | RB-1199…RB-1208 (SYN-0099-01…09, SYN-0100-01) | done: all 10 green first-try, no code changes; 99-double-comma opened+completed (doubled H002 1:19 + missing H003 1:19 refusal-pinned; lonely H002 1:17; trailing H002 1:18 allowed; clean/empty/nested/hetero silent; unterminated E001 EOF 2:1); 100-extra-comma opened (doubled H002 twin) |
| Batch 122 | RB-1209…RB-1218 (SYN-0100-02…09, SYN-0101-01…02) | done: all 10 green first-try, no code changes; 100-extra-comma completed (missing H003 1:19; lonely H002 1:17; trailing H002 1:18; clean/empty/nested/hetero silent; unterminated E001 EOF 2:1); 101-missing-value opened (doubled H002 twin + missing H003) |
| Batch 123 | RB-1219…RB-1228 (SYN-0101-03…09, SYN-0102-01…03) | done: all 10 green first-try, no code changes; 101-missing-value completed (lonely H002 1:17; trailing H002 1:18; clean/empty/nested/hetero silent; unterminated E001 EOF 2:1); 102-unclosed opened (doubled H002 twin + missing H003 + lonely H002) |
| Batch 124 | RB-1229…RB-1238 (SYN-0102-04…09, SYN-0103-01…04) | done: all 10 green individually first-try; whole-file needed 3 runs (1 transient single failure, 3rd occurrence of known flake — residual risk logged); 102-unclosed completed (trailing H002 1:18; clean/empty/nested/hetero silent; unterminated E001 EOF 2:1); 103-incomplete-false opened (fals/tru/nul W001 1:16 + null never coerced; true bool silent) |
| Batch 125 | RB-1239…RB-1248 (SYN-0103-05…09, SYN-0104-01…05) | done: all 10 green first-try, no code changes; 103-incomplete-false completed (false/null/true reserved silent; truex/false_value W001 1:16 never prefix-matched; nested tru W001 1:25); 104-multidigit-then-00 opened (01-decimal; 1.-int; bare/signed-exponent E002 1:17; hex E002 1:17) |
| Batch 126 | RB-1249…RB-1258 (SYN-0104-06…09, SYN-0105-01…06) | done: all 10 green first-try, no code changes; 104-multidigit-then-00 completed (underscore-1000; giant-exponent E002; quoted-digits exact; canonical-zero); 105-number-++ opened (01-decimal; 1.-int; bare/signed-exponent E002 1:17; hex E002 1:17; underscore-1000) |
| Batch 127 | RB-1259…RB-1268 (SYN-0105-07…09, SYN-0106-01…07) | done: all 10 green first-try, no code changes; 105-number-++ completed (giant-exponent E002; quoted-digits exact; canonical-zero); 106-number-minus-01 opened (01-decimal; 1.-int; bare/signed-exponent E002 1:17; hex E002 1:17; underscore-1000; giant-exponent E002) |
| Batch 128 | RB-1269…RB-1278 (SYN-0106-08…09, SYN-0107-01…08) | done: all 10 green first-try, no code changes; 106-number-minus-01 completed (quoted-digits exact; canonical-zero); 107-number-0.1.2 opened (01-decimal; 1.-int; bare/signed-exponent E002 1:17; hex E002 1:17; underscore-1000; giant-exponent E002; quoted-digits exact) |
| Batch 129 | RB-1279…RB-1288 (SYN-0107-09, SYN-0108-01…09) | done: all 10 green first-try, no code changes; 107-number-0.1.2 completed (canonical-zero); 108-number-0e+ opened+completed (01-decimal; 1.-int; bare/signed-exponent E002 1:17; hex E002 1:17; underscore-1000; giant-exponent E002; quoted-digits exact; canonical-zero) |
| Batch 130 | RB-1289…RB-1298 (SYN-0109-01…09, SYN-0110-01) | done: all 10 green first-try, no code changes; 109-number-2.e3 opened+completed (01-decimal; 1.-int; bare/signed-exponent E002 1:17; hex E002 1:17; underscore-1000; giant-exponent E002; quoted-digits exact; canonical-zero); 110-number-Inf opened (01-decimal twin) |
| Batch 131 | RB-1299…RB-1308 (SYN-0110-02…09, SYN-0111-01…02) | done: all 10 green first-try, no code changes; 110-number-Inf completed (1.-int; bare/signed-exponent E002 1:17; hex E002 1:17; underscore-1000; giant-exponent E002; quoted-digits exact; canonical-zero); 111-leading-zero opened (01-decimal twin + 1.-int) |
| Batch 132 | RB-1309…RB-1318 (SYN-0111-03…09, SYN-0112-01…03) | done: all 10 green first-try, no code changes; 111-leading-zero completed (exponent/hex E002 1:17; underscore-1000; quoted-digits exact; canonical-zero); 112-missing-colon opened (quoted-key E001 1:18; keyless E001 1:18; table-trailing H002 1:23) |
| Batch 133 | RB-1319…RB-1328 (SYN-0112-04…09, SYN-0113-01…04) | done: all 10 green first-try, no code changes; 112-missing-colon completed (trailing-garbage E002 1:26; quoted-dup E001 1:18; empty-key E001 1:18; clean/nested/comment tables silent); 113-missing-key opened (quoted E001 1:18 + keyless E001 1:18 + table-trailing H002 1:23 + trailing-garbage E002 1:26) |
| Batch 134 | RB-1329…RB-1338 (SYN-0113-05…09, SYN-0114-01…05) | done: all 10 green first-try, no code changes; 113-missing-key completed (quoted-dup E001 1:18; empty-key E001 1:18; clean/nested/comment silent); 114-trailing-comma opened (quoted E001 1:18 + keyless E001 1:18 + table-trailing H002 1:23 + trailing-garbage E002 1:26 + quoted-dup E001 1:18) |
| Batch 135 | RB-1339…RB-1348 (SYN-0114-06…09, SYN-0115-01…06) | done: all 10 green first-try, no code changes; 114-trailing-comma completed (empty-key E001 1:18; clean/nested/comment silent); 115-unquoted-key opened (quoted E001 1:18 + keyless E001 1:18 + table-trailing H002 1:23 + trailing-garbage E002 1:26 + quoted-dup E001 1:18 + empty-key E001 1:18) |
| Batch 136 | RB-1349…RB-1358 (SYN-0115-07…09, SYN-0116-01…07) | done: all 10 green first-try, no code changes; 115-unquoted-key completed (clean/nested/comment silent); 116-incomplete-escape opened (dangling-backslash H001 1:16; verbatim \u/\x refusals pinned spec/grammar.md:62-66; \n decodes LF; raw-newline refusal per RB-175; emoji+combining exact, no NFC) |
| Batch 137 | RB-1359…RB-1368 (SYN-0116-08…09, SYN-0117-01…08) | done: all 10 green first-try, no code changes; 116-incomplete-escape completed (3-way NUL: verbatim \u0000 / \0 decodes / raw-in-string kept, outside E005 1:6; interp-vs-literal W001 1:19); 117-invalid-unicode-escape opened as exact 116 twins |
| Batch 138 | RB-1369…RB-1378 (SYN-0117-09, SYN-0118-01…09) | done: all 10 green first-try, no code changes; 117 completed (interp twin W001 1:19); 118 opened+completed as third twin lineage (H001 1:16; verbatim refusals; \n LF; raw NL kept; emoji exact; 3-way NUL + E005; interp W001 1:19) |
| Batch 139 | RB-1379…RB-1388 (SYN-0119-01…09, SYN-0120-01) | done: all 10 green first-try, no code changes; 119 opened+completed as fourth twin lineage (H001 1:16; verbatim refusals; \n LF; raw NL kept; emoji exact; 3-way NUL + E005; interp W001 1:19); 120-lonely-surrogate opened (dangling-backslash H001 twin) |
| Batch 140 | RB-1389…RB-1398 (SYN-0120-02…09, SYN-0121-01…02) | done: all 10 green first-try, no code changes, 2 fixtures; 120 continued as fifth twins (verbatim refusals; \n LF; raw NL kept; emoji exact; 3-way NUL + E005; interp W001 1:19); 121 opened (leading NUL E005 1:1; overlong C0 AF E017, converges RB-196) |
| Batch 141 | RB-1399…RB-1408 (SYN-0121-03…09, SYN-0122-01…03) | done: all 10 green first-try, no code changes, 3 fixtures; 121 continued (BOM E005 1:5; verbatim u0001; TAB E006 1:1; cafe/clef exact; trunc-E2 E017; esc-TAB decodes); 122 opened (32 accept, 256/2048 E004 at bound 64) |
| Batch 142 | RB-1409…RB-1418 (SYN-0122-04…09, SYN-0123-01…04) | done: all 10 green first-try, no code changes; 122 continued (array-interp E005 3:10 refusals; cycle inexpressible, S001 control; small array; 64-deep E004 comment-counted; 200KB scalar linear-accept + OPEN SPEC QUESTION: scalar byte cap, spec/budgets.md silent); 123 opened as 122 twins |
| Batch 143 | RB-1419…RB-1428 (SYN-0123-05…09, SYN-0124-01…05) | done: all 10 green first-try, no code changes; 123 continued as 122 twins (cycle E005, S001 control; small array; 64-deep E004 counted; fan-out E005; 200KB accept); 124 opened (flat/nested W011 last-wins refusals P1-TI-012; quoted-vs-bare E001 1:18; distinct silent; U+00E9 pair E001, no NFC) |
| Batch 144 | RB-1429…RB-1438 (SYN-0124-06…09, SYN-0125-01…06) | done: all 10 green first-try, no code changes; 124 completed (distinct silent; naive patch/remove E019 3:2 with call-form controls; dotted-quoted E001 1:18); 125 opened as sixth twins (H001 1:16; verbatim refusals; \n LF; raw NL kept) |
| Batch 145 | RB-1439…RB-1448 (SYN-0125-07…09, SYN-0126-01…07) | done: all 10 green first-try, no code changes; 125 completed as sixth twins; 126 opened as number lineage (01 decimal + 1. int refusals P1-TI-004; 1e/1e+/hex/giant E002 1:17; 1_000 canonical); one transient rb_086 WallClock flake, green on re-run (suite 2001 exit 0) |
| Batch 146 | RB-1449…RB-1458 (SYN-0126-08…09, SYN-0127-01…08) | done: all 10 green first-try, no code changes, gates first-try (suite 2011 exit 0); 126 completed (quoted-01 exact; canonical zero); 127 opened as number twins (01/1. refusals; E002 edges; 1_000; quoted-01) |
| Batch 147 | RB-1459…RB-1468 (SYN-0127-09, SYN-0128-01…09) | done: all 10 green first-try, no code changes, gates first-try (suite 2021 exit 0); 127 completed (canonical zero); 128 opened as table rhythm (quoted/keyless E001 1:18; trailing H002 1:23; garbage E002 1:26; quoted-dup/empty-key E001; clean/nested/comment silent) |
| Batch 148 | RB-1469…RB-1478 (SYN-0129-01…09, SYN-0130-01) | done: all 10 green first-try, no code changes, gates first-try (suite 2031 exit 0); 129 opened as table rhythm (quoted/keyless E001 1:18; trailing H002 1:23; garbage E002 1:26; quoted-dup/empty-key E001; clean/nested/comment silent); 130 opened (bare yes W001 1:16 + null refusal, never bool) |
| Batch 149 | RB-1479…RB-1488 (SYN-0130-02…09, SYN-0131-01…02) | done: all 10 green first-try, no code changes, gates first-try (suite 2041 exit 0); 130 continued (no/on/off W001 1:16 + null refusals; true/false bool; FALSE W001; null; quoted-no exact; 1.0 float); 131 opened (baseline + nested silent) |
| Batch 150 (FINAL: 150 cap) | RB-1489…RB-1498 (SYN-0131-03…09, SYN-0132-01…03) | done: all 10 green first-try, no code changes, gates first-try (suite 2051 exit 0); 131 completed (array/string/comment/env/secret-refusal-free/interp-E005-3:14/unit-ms5000); 132 opened as seventh twins (H001 1:16; verbatim refusals). 150-BATCH CAP REACHED, goal closing per human instruction. |
| Batch 151 | RB-1499…RB-1508 (SYN-0132-04…09, SYN-0133-01…04) | done: all 10 green first-try, no code changes, gates first-try (suite 2061 exit 0); 132 completed as seventh twins; 133 opened (brace baselines silent; equals-indent E002 1:7 refusals with colon-twin equality proof RB-102) |
| Byte fixtures (mined) | headed cases → tests/research/*.active + RB IDs | mined per batch, ledger cites |

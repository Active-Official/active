# Active unit reservation record

Every suffix Active will ever lex is decided here. Single ASCII letters are
exhausted; new dimensions use long-form-first plus collision-free shorts.
`m s B h d` are frozen forever. Case-SENSITIVE always.

## Admitted (lexer whitelist + grammar.md §1.6 + spec Ch. 4)

| Suffix | Dimension | Canonical | Arithmetic | Export |
|---|---|---|---|---|
| `ns us µs ms s m h d` | time | `DurationMs` ms | full (add/sub, scalar, ratio, compare) | `{"ms": N}` / `Nms` |
| `week sec min` | time aliases (`week` exact 604800000ms; plurals refused) | `DurationMs` ms | full (inherited) | `{"ms": N}` / `Nms` |
| `B KiB MiB GiB TiB KB MB GB TB` | bytes | `Int` bytes saturating | plain-int ops | bare int |
| `PiB EiB` | large binary bytes | `Int` bytes saturating (16EiB → i64::MAX) | plain-int ops | bare int |
| `/s /m /h per_s per_m per_h req/s req_s /req/s /req/m /req/h req/m req_m req/h req_h` | rate | retained `Quantity` | same-suffix add/sub/compare/scale | `{"<suffix>": N}` / `N<suffix>` |
| `%` | percent (dimensionless) | `Quantity` ratio, 100% = 1.0 | same-dimension add/sub/compare/scale, no implicit base | `{"%": ratio}` / percent-form `25%` |
| `Hz kHz MHz GHz THz` | frequency | `Quantity` f64 hertz | full (add/sub, scalar, ratio, compare) | `{"Hz": N}` / `NHz` |
| `b Kb Mb Gb` | bits (distinct from bytes) | `Quantity` f64 bits | full within dimension | `{"b": N}` / `Nb` |
| `bps Kbps Mbps Gbps MB/s` | data rates | `Quantity` f64 bits/sec (`MB/s` ×8) | full within dimension | `{"bps": N}` / `Nbps` |
| `tokens ktokens Mtokens` | token counts (AI) | `Quantity` f64 tokens | full within dimension | `{"tokens": N}` / `Ntokens` |
| `tok/s` | inference throughput (AI) | `Quantity` f64 tok/s | full within dimension | `{"tok/s": N}` / `Ntok/s` |
| `FLOPS TFLOPS PFLOPS` | compute accounting (AI) | `Quantity` f64 flop/s | full within dimension | `{"FLOPS": N}` / `NFLOPS` |
| `Pa bar psi` | pressure | `Quantity` f64 Pa | full within dimension | `{"Pa": N}` / `NPa` |
| `V A W` | electrical | `Quantity` f64 V/A/W | full within dimension (V+A refuses) | `{"V"/"A"/"W": N}` |
| `J kWh` | energy | `Quantity` f64 J | full within dimension | `{"J": N}` / `NJ` |
| `N` | force | `Quantity` f64 N | full within dimension | `{"N": N}` / `NN` |
| `deg rad` | angle | `Quantity` f64 rad | full within dimension | `{"rad": N}` / `Nrad` |
| `rpm` | rotation rate | `Quantity` f64 rpm (retained) | full within dimension | `{"rpm": N}` / `Nrpm` |
| `meter km` | length (long-form only) | `Quantity` f64 m | full within dimension | `{"meter": N}` / `Nmeter` |
| `kg g` | mass | `Quantity` f64 kg | full within dimension | `{"kg": N}` / `Nkg` |
| `K Cel Fahr` | temperature (affine special) | `Quantity` f64 K | equality + ordering ONLY | `{"K": N}` / `NK` |
| `dB` | decibels (log special) | `Quantity` f64 dB retained | equality + ordering ONLY | `{"dB": N}` / `NdB` |

## Rejected near-misses (parse E002/E005, never silent)

| Spelling | Reason |
|---|---|
| `m` for meters / millicores | frozen = minutes; k8s contextual typing refused |
| `25 %` (spaced) | space = two tokens (E005); adjacency is load-bearing |
| `y mo month year` | no single meaning (365 vs 365.25 vs tropical) |
| bare numbers as quantities | implicit defaults refused (Norway lineage) |
| `infinity` durations | sentinel smuggled into a dimension |
| currency suffixes | jurisdiction semantics out of scope |
| case variants (`MS`, `S`) | case discipline; `MS` ≠ `ms` is a feature |

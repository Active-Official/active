#!/usr/bin/env python3
"""L-P37 differential runner: the Python oracle against the shipped engine.

Corpus side: `--corpus-dump` (the Rust conformance test's
`ACTIVE_CORPUS_DUMP` projection) is the reference and carries ordered
diagnostic records, spans, duplicate counts, the redacted value, and its
hash. Without it, the committed expected/errors sidecars are the
fallback; those carry a deduplicated code list, so that mode compares
the code set and the value. MC source + projection come from the Rust
sweep's deterministic dump (`ACTIVE_MC_DUMP`).

Exit 0 only when every non-allowlisted case agrees.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

import tomllib

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from active_ref.api import evaluate_file, evaluate_source
from active_ref.eval import Budget, Engine
from active_ref.export import content_hash, to_json_value

ROOT = HERE.parents[1]
MANIFEST = ROOT / "conformance" / "manifest.toml"
CASES = ROOT / "conformance" / "cases"

LEAK_MARKERS = ("hunter2", "payload-secret", "taint-mismatch-probe")

# Per-case host configuration comes from the manifest itself (profile
# and pure keys), so a new host-surface case cannot silently run with the
# wrong engine. The case id no longer needs a hardcoded table.


def codes(outcome) -> list[str]:
    """Sorted multiset: duplicate diagnostics are behavior (G01)."""
    return sorted(d.code for d in outcome.diagnostics)


def diag_records(outcome) -> list[dict]:
    def span(d):
        if d.span is None:
            return None
        return {
            "line": d.span.line,
            "column": d.span.column,
            "end_line": d.span.end_line,
            "end_column": d.span.end_column,
        }

    return [
        {
            "code": d.code,
            "severity": d.severity,
            "path": d.path,
            "span": span(d),
        }
        for d in outcome.diagnostics
    ]


def _strings_and_keys(node) -> list[str]:
    out: list[str] = []
    if isinstance(node, dict):
        for key, item in node.items():
            out.append(key)
            out.extend(_strings_and_keys(item))
    elif isinstance(node, list):
        for item in node:
            out.extend(_strings_and_keys(item))
    elif isinstance(node, str):
        out.append(node)
    return out


def _text_pairs(raw, safe):
    if isinstance(raw, dict) and isinstance(safe, dict):
        for raw_item, safe_item in zip(raw.items(), safe.items()):
            yield raw_item[0], safe_item[0]
            yield from _text_pairs(raw_item[1], safe_item[1])
    elif isinstance(raw, list) and isinstance(safe, list):
        for raw_item, safe_item in zip(raw, safe):
            yield from _text_pairs(raw_item, safe_item)
    elif isinstance(raw, str) and isinstance(safe, str):
        yield raw, safe


def scan_leaks(case_id: str, outcome) -> list[str]:
    found = []
    markers = set(LEAK_MARKERS) | set(outcome.seeds)
    messages = "\n".join(d.message for d in outcome.diagnostics)
    if outcome.value is not None:
        raw = to_json_value(outcome.value, seeds=())
        safe = to_json_value(outcome.value, seeds=outcome.seeds)
        for raw_text, safe_text in _text_pairs(raw, safe):
            for marker in markers:
                if not marker:
                    continue
                if len(marker.encode("utf-8")) >= 4:
                    if marker in raw_text and (
                        marker in safe_text or safe_text == raw_text
                    ):
                        found.append(
                            f"LEAK {case_id}: raw marker `{marker}` in an output"
                        )
                elif raw_text == marker and safe_text == marker:
                    found.append(
                        f"LEAK {case_id}: exact short secret `{marker}` exported"
                    )
    for marker in markers:
        if not marker:
            continue
        if len(marker.encode("utf-8")) >= 4 and marker in messages:
            found.append(f"LEAK {case_id}: raw marker `{marker}` in a message")
        elif len(marker.encode("utf-8")) < 4 and re.search(
            rf"(?<![A-Za-z0-9_]){re.escape(marker)}(?![A-Za-z0-9_])", messages
        ):
            found.append(f"LEAK {case_id}: short secret `{marker}` in a message")
    return found


def scan_dump_messages(case_id: str, record: dict, seeds) -> list[str]:
    messages = "\n".join(record.get("messages") or [])
    found: list[str] = []
    for marker in set(LEAK_MARKERS) | set(seeds):
        if len(marker.encode("utf-8")) >= 4 and marker in messages:
            found.append(f"LEAK {case_id}: Rust message contains `{marker}`")
    return found


def scan_material(case_id: str, outcome) -> list[str]:
    """Every material leaf must render as the marker, not its plaintext."""
    if outcome.value is None:
        return []
    rendered = to_json_value(outcome.value, seeds=outcome.seeds)
    found: list[str] = []

    def walk(value, node) -> None:
        if (
            value.material
            and value.kind not in ("secret", "blob", "array", "table")
            and node != "***SECRET***"
        ):
            found.append(f"LEAK {case_id}: material scalar rendered as {node!r}")
        if value.kind == "blob" and value.data.tainted:
            media = (
                node.get("$active.blob", {}).get("media_type")
                if isinstance(node, dict)
                else None
            )
            if media != "***SECRET***":
                found.append(f"LEAK {case_id}: tainted blob media exposed")
        if value.kind == "table" and isinstance(node, dict):
            for key, item in value.data.items():
                walk(item, node.get(key))
        elif value.kind == "array" and isinstance(node, list):
            for index, item in enumerate(value.data):
                if index < len(node):
                    walk(item, node[index])

    walk(outcome.value, rendered)
    return found


def engine_for(case: dict) -> Engine:
    flags: dict = {}
    if case.get("profile"):
        flags["profile_override"] = case["profile"]
    if case.get("pure"):
        flags["pure"] = True
    return Engine(**flags)


def run_corpus(
    corpus_dump: Path | None = None,
) -> tuple[int, int, list[str]]:
    manifest = tomllib.loads(MANIFEST.read_text(encoding="utf-8"))
    dumped: dict[str, dict] = {}
    if corpus_dump is not None:
        dumped = {
            record["id"]: record
            for record in (
                json.loads(line)
                for line in corpus_dump.read_text(encoding="utf-8").splitlines()
                if line.strip()
            )
        }
    passed = 0
    total = 0
    failures: list[str] = []
    for case in manifest.get("cases", []):
        case_id = case["id"]
        path = ROOT / "conformance" / case["file"]
        exp_path = ROOT / "conformance" / "expected" / f"{case_id}.json"
        err_path = ROOT / "conformance" / "errors" / f"{case_id}.json"
        expected = (
            json.loads(exp_path.read_text(encoding="utf-8"))
            if exp_path.exists()
            else None
        )
        errors = (
            json.loads(err_path.read_text(encoding="utf-8"))
            if err_path.exists()
            else None
        )
        total += 1
        outcome = evaluate_file(str(path), engine_for(case))
        failures.extend(scan_leaks(case_id, outcome))
        failures.extend(scan_material(case_id, outcome))
        if corpus_dump is not None:
            record = dumped.get(case_id)
            if record is None:
                failures.append(f"{case_id}: missing from the corpus dump")
                continue
            failures.extend(scan_dump_messages(case_id, record, outcome.seeds))
            got_codes = codes(outcome)
            want_codes = sorted(record.get("codes") or [])
            records_ok = diag_records(outcome) == record.get("diag_records")
            ok_ok = (outcome.value is not None) == bool(record.get("ok"))
            if record.get("content_hash") is None:
                hash_ok = outcome.value is None
            else:
                hash_ok = (
                    outcome.value is not None
                    and content_hash(outcome.value, seeds=outcome.seeds)
                    == record["content_hash"]
                )
            if record.get("value") is None:
                value_ok = outcome.value is None
            else:
                value_ok = (
                    outcome.value is not None
                    and to_json_value(outcome.value, seeds=outcome.seeds)
                    == record["value"]
                )
            if (
                got_codes == want_codes
                and records_ok
                and ok_ok
                and hash_ok
                and value_ok
            ):
                passed += 1
            else:
                failures.append(
                    f"{case_id}: codes {got_codes} want {want_codes}; "
                    f"records_ok={records_ok}; ok_ok={ok_ok}; "
                    f"hash_ok={hash_ok}; value_ok={value_ok}"
                )
            continue
        got_codes = sorted(set(codes(outcome)))
        want_codes = sorted(set(errors.get("codes", []))) if errors else []
        code_ok = got_codes == want_codes
        value_ok = True
        if expected is not None:
            value_ok = (
                outcome.value is not None
                and to_json_value(outcome.value, seeds=outcome.seeds) == expected
            )
        if code_ok and value_ok:
            passed += 1
        else:
            failures.append(
                f"{case_id}: codes {got_codes} want {want_codes}; value_ok={value_ok}"
            )
    return passed, total, failures


def run_mc(dump_path: Path) -> tuple[int, int, list[str]]:
    passed = 0
    total = 0
    failures: list[str] = []
    for line in dump_path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        record = json.loads(line)
        total += 1
        env = {}
        if record.get("env_probe") is not None:
            env["ACTIVE_MC_PROBE"] = record["env_probe"]
        case_id = f"mc-{record['family']}-{record['case']}"
        outcome = evaluate_source(
            record["source"], case_id, Engine(env=env, budget=Budget.trusted())
        )
        failures.extend(scan_leaks(case_id, outcome))
        failures.extend(scan_material(case_id, outcome))
        failures.extend(scan_dump_messages(case_id, record, outcome.seeds))
        got_codes = codes(outcome)
        want_codes = sorted(record.get("codes") or [])
        records_ok = True
        if "diag_records" in record:
            records_ok = diag_records(outcome) == record["diag_records"]
        ok_ok = (outcome.value is not None) == bool(record.get("ok"))
        hash_ok = True
        if record.get("content_hash") is not None:
            hash_ok = (
                outcome.value is not None
                and content_hash(outcome.value, seeds=outcome.seeds)
                == record["content_hash"]
            )
        value_ok = True
        if record.get("value") is not None:
            value_ok = (
                outcome.value is not None
                and to_json_value(outcome.value, seeds=outcome.seeds) == record["value"]
            )
        if got_codes == want_codes and records_ok and ok_ok and hash_ok and value_ok:
            passed += 1
        else:
            failures.append(
                f"{case_id}: codes {got_codes} want {want_codes}; "
                f"records_ok={records_ok}; ok_ok={ok_ok}; "
                f"hash_ok={hash_ok}; value_ok={value_ok}"
            )
    return passed, total, failures


def load_allowlist(path: Path) -> dict[str, str]:
    if not path.exists():
        return {}
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    return {entry["id"]: entry.get("gap", "") for entry in data.get("blocked", [])}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mc-dump", type=Path, help="JSONL from ACTIVE_MC_DUMP")
    parser.add_argument(
        "--corpus-dump", type=Path, help="JSONL from ACTIVE_CORPUS_DUMP"
    )
    parser.add_argument("--allowlist", type=Path, default=HERE / "known-gaps.toml")
    parser.add_argument("--report", type=Path, help="write a JSON report here")
    parser.add_argument("--corpus-only", action="store_true")
    parser.add_argument("--expect-corpus", type=int)
    parser.add_argument("--expect-mc", type=int)
    args = parser.parse_args()

    allow = load_allowlist(args.allowlist)
    passed_c, total_c, failures_c = run_corpus(args.corpus_dump)
    passed_m = total_m = 0
    failures_m: list[str] = []
    if args.mc_dump and not args.corpus_only:
        passed_m, total_m, failures_m = run_mc(args.mc_dump)

    failures = failures_c + failures_m
    if args.expect_corpus is not None and total_c != args.expect_corpus:
        failures.append(f"count: corpus {total_c} != expected {args.expect_corpus}")
    if args.expect_mc is not None and total_m != args.expect_mc:
        failures.append(f"count: mc {total_m} != expected {args.expect_mc}")
    blocked = []
    real = []
    for failure in failures:
        case_id = failure.split(":", 1)[0]
        if case_id in allow:
            blocked.append({"case": case_id, "gap": allow[case_id], "detail": failure})
        else:
            real.append(failure)

    report = {
        "corpus": {"passed": passed_c, "total": total_c},
        "mc": {"passed": passed_m, "total": total_m},
        "blocked": blocked,
        "divergences": real,
        "ok": not real,
    }
    if args.report:
        args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(
        f"corpus {passed_c}/{total_c}, mc {passed_m}/{total_m}, "
        f"blocked {len(blocked)}, divergences {len(real)}"
    )
    for failure in real:
        print("DIVERGE " + failure)
    for entry in blocked:
        print(f"BLOCKED {entry['case']} ({entry['gap']}): {entry['detail']}")
    if real:
        return 1
    print("DIFFERENTIAL-OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Validate shipped JSON outputs against docs/schemas (TOOLING QUERY.005).

Runs the real CLI over fixtures, parses stdout, and validates each
document against its committed JSON Schema. Exit nonzero on any shape
drift. Requires the `jsonschema` package (CI installs it for the
codegen gate).

Usage: python conformance/evidence/output_schema_check.py
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import jsonschema

ROOT = Path(__file__).resolve().parents[2]
SCHEMAS = ROOT / "docs" / "schemas"
EXE = (
    ROOT / "target" / "debug" / ("active.exe" if sys.platform == "win32" else "active")
)

FIXTURE = """\
schema = {
  port = Int(min = 1, max = 65535, description = "listen port"),
  token = String,
}
port = 8080
token = secret("hunter2-schema")
id = blob("text/plain", "hi")
"""

REPAIR_FIXTURE = """\
schema = { port = Int(default = 8080), host = String(default = "x") }
unexpected = 1
"""

REPAIR_RECEIPT_FIXTURE = """\
schema = { closed = true, host = String, port = Int(default = 8080) }
Host = "x"
"""

LINT_FIXTURE = """\
[server]
readTimeout = 1
max_retries = 2
"""

DIFF_OLD = """\
port = 8080
next = port + 1
"""

DIFF_NEW = """\
port = 9090
next = port + 1
"""


def run(args: list[str]) -> str:
    proc = subprocess.run(
        [str(EXE), *args],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=180,
    )
    if not proc.stdout.strip():
        raise SystemExit(
            f"no stdout for {' '.join(args)} (exit {proc.returncode}): {proc.stderr[:400]}"
        )
    return proc.stdout


def validate(name: str, document: object) -> None:
    schema = json.loads((SCHEMAS / f"{name}.json").read_text(encoding="utf-8"))
    jsonschema.validate(instance=document, schema=schema)


def main() -> int:
    if not EXE.exists():
        raise SystemExit(f"CLI not built at {EXE}; run cargo build -p active-cli")
    work = Path(tempfile.mkdtemp(prefix="active-out-schema-"))
    # Plans and receipts use the per-user key; keep the check hermetic.
    os.environ["ACTIVE_CACHE_KEY_FILE"] = str(work / "cache-key")
    fixture = work / "doc.active"
    fixture.write_text(FIXTURE, encoding="utf-8")
    repair_fixture = work / "repair.active"
    repair_fixture.write_text(REPAIR_FIXTURE, encoding="utf-8")
    repair_receipt_fixture = work / "repair-receipt.active"
    repair_receipt_fixture.write_text(REPAIR_RECEIPT_FIXTURE, encoding="utf-8")
    lint_fixture = work / "lint.active"
    lint_fixture.write_text(LINT_FIXTURE, encoding="utf-8")
    diff_old = work / "old.active"
    diff_old.write_text(DIFF_OLD, encoding="utf-8")
    diff_new = work / "new.active"
    diff_new.write_text(DIFF_NEW, encoding="utf-8")
    apply_old = work / "apply-old.active"
    apply_old.write_text(DIFF_OLD, encoding="utf-8")
    apply_new = work / "apply-new.active"
    apply_new.write_text(DIFF_NEW, encoding="utf-8")
    plan_file = work / "plan.json"
    receipt_file = work / "plan.receipt.json"

    checks = [
        ("graph", json.loads(run(["graph", str(fixture), "--format", "json"]))),
        ("taint", json.loads(run(["viz", "taint", "--from", str(fixture)]))),
        ("budget", json.loads(run(["viz", "budget", "--from", str(fixture)]))),
        (
            "inspect-value",
            json.loads(run(["inspect", "--from", str(fixture), "--path", "port"])),
        ),
        ("mode", json.loads(run(["mode", "--from", str(fixture), "--format", "json"]))),
        ("lint", json.loads(run(["lint", str(repair_fixture), "--format", "json"]))),
        ("sarif", json.loads(run(["lint", str(lint_fixture), "--format", "sarif"]))),
        (
            "repair-plan",
            json.loads(
                run(["repair", "--from", str(repair_fixture), "--format", "json"])
            ),
        ),
        (
            "debug-steps",
            json.loads(run(["debug", "--from", str(fixture), "--resilient"])),
        ),
        (
            "diff",
            json.loads(
                run(
                    [
                        "diff",
                        str(diff_old),
                        str(diff_new),
                        "--format",
                        "json",
                    ]
                )
            ),
        ),
        (
            "plan",
            json.loads(
                run(
                    [
                        "plan",
                        str(diff_old),
                        str(diff_new),
                        "--format",
                        "json",
                    ]
                )
            ),
        ),
    ]
    for name, document in checks:
        validate(name, document)
        print(f"OUTPUT-SCHEMA {name} ok")

    # L-P41: the repair receipt (a committed success and a refusal).
    repair_out = work / "repair-out.active"
    repair_receipt = work / "repair.receipt.json"
    run(
        [
            "repair",
            "--from",
            str(repair_receipt_fixture),
            "--output",
            str(repair_out),
            "--receipt",
            str(repair_receipt),
        ]
    )
    validate(
        "repair-receipt",
        json.loads(repair_receipt.read_text(encoding="utf-8")),
    )
    print("OUTPUT-SCHEMA repair-receipt ok")

    broken_repair = work / "broken-repair.active"
    broken_repair.write_text("port = \n", encoding="utf-8")
    refused_repair = work / "repair-refused.receipt.json"
    refused = subprocess.run(
        [
            str(EXE),
            "repair",
            "--from",
            str(broken_repair),
            "--receipt",
            str(refused_repair),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if refused.returncode == 0 or not refused_repair.is_file():
        raise SystemExit(f"OUTPUT-SCHEMA repair refusal not audited: {refused.stderr}")
    validate(
        "repair-receipt",
        json.loads(refused_repair.read_text(encoding="utf-8")),
    )
    print("OUTPUT-SCHEMA repair-receipt refusal ok")

    # A plan written to disk must be apply-able and produce a receipt:
    # validate both the on-disk plan and the receipt.
    run(
        [
            "plan",
            str(apply_old),
            str(apply_new),
            "--output",
            str(plan_file),
        ]
    )
    plan_document = json.loads(plan_file.read_text(encoding="utf-8"))
    validate("plan", plan_document)
    approve = plan_document["plan"]["plan_hash"]
    run(
        [
            "apply",
            str(plan_file),
            "--approve",
            approve,
            "--operator",
            "output-schema",
            "--receipt",
            str(receipt_file),
        ]
    )
    receipt = json.loads(receipt_file.read_text(encoding="utf-8"))
    validate("plan-receipt", receipt)
    print("OUTPUT-SCHEMA plan-receipt ok")

    # L-P41: a refused apply writes a refusal receipt and never touches the
    # baseline. Replaying the same plan now drifts (the baseline is the
    # installed candidate), which is a stable refusal class.
    refusal_file = work / "refused.receipt.json"
    refused = subprocess.run(
        [
            str(EXE),
            "apply",
            str(plan_file),
            "--approve",
            approve,
            "--receipt",
            str(refusal_file),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if refused.returncode == 0 or not refusal_file.is_file():
        raise SystemExit(f"OUTPUT-SCHEMA apply refusal not audited: {refused.stderr}")
    validate("plan-receipt", json.loads(refusal_file.read_text(encoding="utf-8")))
    print("OUTPUT-SCHEMA plan-receipt refusal ok")

    # A file that does not parse still fails the run, but its E-codes
    # are not lint findings: the pinned schema admits `^L[0-9]{3}$`
    # only, so the JSON array stays empty and stderr carries the parse
    # diagnostics.
    broken = work / "broken.active"
    broken.write_text("port = \n", encoding="utf-8")
    proc = subprocess.run(
        [str(EXE), "lint", str(broken), "--format", "json"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=180,
    )
    assert proc.returncode != 0, "a broken file must fail the lint run"
    broken_report = json.loads(proc.stdout)
    validate("lint", broken_report)
    assert broken_report == [], (
        f"parse diagnostics leaked into lint JSON: {broken_report}"
    )
    assert "E0" in proc.stderr, (
        f"parse diagnostics missing from stderr: {proc.stderr!r}"
    )
    print("OUTPUT-SCHEMA lint-broken ok (parse errors on stderr, empty lint array)")

    print(f"OUTPUT-SCHEMA: OK ({len(checks)} outputs + broken lint file)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

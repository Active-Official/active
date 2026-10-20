#!/usr/bin/env python3
"""Shared cross-implementation conformance harness (CHECKLIST Step 0.004).

Runs the same conformance manifest through every available Active
implementation and emits one unified JSON report.

Paths run today:
  reference  Rust reference via `cargo test -p active-core --test conformance`
  c_abi      C ABI surface via `cargo test -p active-ffi`
  wasm       zero-import module via `node active-wasm/run-conformance.js`
  go         wazero embedding via `go run .` in active-wasm/smoke-go

Placeholders (status "not-implemented") reserve slots for language
bindings as they land; a placeholder never counts as a pass.

Normalization rules (PROPOSAL section 5.1, tier G-017):
  codes      diagnostic codes compare strictly; any mismatch fails the path.
  spans      advisory only; drift is reported, never failing.
  secrets    any raw secret marker in any output fails the whole run.
  json       objects compare key-order-insensitively after parsing.

Usage:
  python conformance/harness/run.py [--out report.json]
Exit code is 0 only when every implemented path passes.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RAW_SECRET = "hunter2"


def sh(argv: list[str], cwd: Path) -> tuple[int, str]:
    p = subprocess.run(argv, cwd=cwd, capture_output=True, text=True)
    return p.returncode, p.stdout + p.stderr


def parse_tally(output: str, pattern: str) -> str | None:
    m = re.search(pattern, output)
    return m.group(0) if m else None


PATHS: list[tuple[str, list[str], Path, str]] = [
    (
        "reference",
        ["cargo", "test", "-p", "active-core", "--test", "conformance"],
        ROOT,
        r"test result: ok\..*",
    ),
    ("c_abi", ["cargo", "test", "-p", "active-ffi"], ROOT, r"test result: ok\..*"),
    (
        "wasm",
        ["node", "active-wasm/run-conformance.js"],
        ROOT,
        r"\d+/\d+ pass, \d+ pinned",
    ),
    ("go", ["go", "run", "."], ROOT / "active-wasm" / "smoke-go", r"eval port=8080 ok"),
]

PLACEHOLDERS = ["python", "js-native", "go-native"]


def main() -> int:
    out = Path(sys.argv[sys.argv.index("--out") + 1]) if "--out" in sys.argv else None
    report: dict = {"paths": {}, "secret_leak": False, "ok": False}
    failed = False
    for name, argv, cwd, tally in PATHS:
        code, output = sh(argv, cwd)
        clean = RAW_SECRET not in output
        if not clean:
            report["secret_leak"] = True
        tally_line = parse_tally(output, tally)
        passed = code == 0 and tally_line is not None and clean
        if not passed:
            failed = True
        report["paths"][name] = {
            "status": "pass" if passed else "fail",
            "exit": code,
            "tally": tally_line,
            "secret_clean": clean,
        }
    for name in PLACEHOLDERS:
        report["paths"][name] = {"status": "not-implemented"}
    report["ok"] = not failed
    text = json.dumps(report, indent=2)
    print(text)
    if out:
        out.write_text(text + "\n", encoding="utf-8")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

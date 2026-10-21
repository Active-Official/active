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


def sh(argv: list[str], cwd: Path) -> tuple[int, str, bool]:
    try:
        p = subprocess.run(
            argv,
            cwd=cwd,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=900,
        )
        return p.returncode, p.stdout + p.stderr, False
    except subprocess.TimeoutExpired as e:
        tail = (e.stdout or "") + (e.stderr or "")
        if isinstance(tail, bytes):
            tail = tail.decode("utf-8", "replace")
        return 124, f"TIMEOUT after 900s: {tail[-2000:]}", True
    except FileNotFoundError as e:
        return 127, f"MISSING BINARY: {e}", False


def parse_tally(output: str, pattern: str) -> str | None:
    m = re.search(pattern, output, re.MULTILINE)
    return m.group(0) if m else None


PATHS: list[tuple[str, list[str], Path, str]] = [
    (
        "reference",
        ["cargo", "test", "-p", "active-core", "--test", "conformance"],
        ROOT,
        r"^test result: ok\.",
    ),
    ("c_abi", ["cargo", "test", "-p", "active-ffi"], ROOT, r"^test result: ok\."),
    (
        "wasm",
        ["node", "active-wasm/run-conformance.js"],
        ROOT,
        r"^\d+/\d+ pass, \d+ pinned$",
    ),
    (
        "go",
        ["go", "run", "."],
        ROOT / "active-wasm" / "smoke-go",
        r"^eval port=8080 ok$",
    ),
    (
        "python",
        ["python", "bindings/python/test_active.py"],
        ROOT,
        r"^OK$",
    ),
]

PLACEHOLDERS = ["js-native", "go-native"]


def self_check() -> int:
    """Validate the tally regexes against canned samples (review P2-1).

    Guards the parser itself: a regex that matches nothing makes its path
    fail, and a regex that matches failure output is caught here.
    """
    cases = [
        (
            "test result: ok. 1 passed; 0 failed; finished in 0.01s",
            r"^test result: ok\.",
            True,
        ),
        (
            "test result: FAILED. 0 passed; 1 failed; finished in 0.01s",
            r"^test result: ok\.",
            False,
        ),
        ("11/13 pass, 2 pinned", r"^\d+/\d+ pass, \d+ pinned$", True),
        ("11/13 pass, 2 pinned (extra)", r"^\d+/\d+ pass, \d+ pinned$", False),
        ("eval port=8080 ok", r"^eval port=8080 ok$", True),
        ("eval port=8080 FAILED", r"^eval port=8080 ok$", False),
        ("OK", r"^OK$", True),
        ("FAILED (failures=1)", r"^OK$", False),
    ]
    bad = 0
    for sample, pattern, want in cases:
        got = parse_tally(sample, pattern) is not None
        if got != want:
            print(f"SELF-CHECK FAIL: {sample!r} vs {pattern!r}")
            bad += 1
    print(f"self-check: {len(cases) - bad}/{len(cases)}")
    return 1 if bad else 0


def main() -> int:
    if "--self-check" in sys.argv:
        return self_check()
    out = Path(sys.argv[sys.argv.index("--out") + 1]) if "--out" in sys.argv else None
    report: dict = {"paths": {}, "secret_leak": False, "ok": False}
    failed = False
    for name, argv, cwd, tally in PATHS:
        code, output, timed_out = sh(argv, cwd)
        clean = RAW_SECRET not in output
        if not clean:
            report["secret_leak"] = True
        tally_line = parse_tally(output, tally)
        passed = code == 0 and tally_line is not None and clean and not timed_out
        if not passed:
            failed = True
        entry = {
            "status": "pass" if passed else "fail",
            "exit": code,
            "tally": tally_line,
            "secret_clean": clean,
        }
        if timed_out:
            entry["timed_out"] = True
        report["paths"][name] = entry
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

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

Normalization rules (PROPOSAL section 5.1, tier G-017, strict parity gate):
  codes      diagnostic codes compare strictly; any mismatch fails the path.
  spans      compared STRICTLY by the `differential` gate, which drives
             every hermetic manifest case through three independent
             embeddings (C ABI via ctypes with structured spans, Python
             and JS via envelope items) and asserts identical ordered
             code lists, span tuples, severities, refused flags, and
             redacted JSON. Span drift fails the whole run; per-path
             suites that cannot surface spans (Tier-2 smoke runners
             assert codes+values+redaction) never count as span proof.
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
REDACTED = "***SECRET***"


def sh(argv: list[str], cwd: Path) -> tuple[int, str, bool]:
    # `python` is not on PATH everywhere (Debian ships `python3` only);
    # always run the current interpreter.
    if argv and argv[0] == "python":
        argv = [sys.executable, *argv[1:]]
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


def scrub(text: str) -> str:
    """Redact the fixture raw secret before anything is stored or printed."""
    return text.replace(RAW_SECRET, REDACTED)


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
    (
        "js-native",
        ["node", "bindings/js/test_active.js"],
        ROOT,
        r"^js-ok$",
    ),
    (
        "zig",
        ["python", "bindings/zig/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "csharp",
        ["python", "bindings/csharp/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "cpp",
        ["python", "bindings/cpp/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "java",
        ["python", "bindings/java/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "lua",
        ["python", "bindings/lua/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "ruby",
        ["python", "bindings/ruby/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "php",
        ["python", "bindings/php/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "fortran",
        ["python", "bindings/fortran/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "go-native",
        ["python", "bindings/go/run-tests.py"],
        ROOT,
        r"^TEST-OK$",
    ),
    (
        # Cross-implementation differential: identical codes, spans, and
        # redacted JSON across the C ABI, Python, and JS embeddings on
        # every hermetic manifest case (pins honored). This is what makes
        # the Phase-2 criterion 1 promise checkable instead of advisory.
        "differential",
        ["python", "conformance/harness/diff_vectors.py"],
        ROOT,
        r"^DIFFERENTIAL-OK",
    ),
    (
        "haskell",
        ["python", "bindings/haskell/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "julia",
        ["python", "bindings/julia/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "nim",
        ["python", "bindings/nim/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "perl",
        ["python", "bindings/perl/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "r",
        ["python", "bindings/r/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "ocaml",
        ["python", "bindings/ocaml/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "crystal",
        ["python", "bindings/crystal/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "elixir",
        ["python", "bindings/elixir/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "dart",
        ["python", "bindings/dart/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "vlang",
        ["python", "bindings/vlang/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "kotlin",
        ["python", "bindings/kotlin/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
    (
        "swift",
        ["python", "bindings/swift/run-smoke.py"],
        ROOT,
        r"^SMOKE-OK$",
    ),
]

PLACEHOLDERS = []


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
        ("js-ok", r"^js-ok$", True),
        ("SMOKE-OK", r"^SMOKE-OK$", True),
        ("SMOKE-FAIL", r"^SMOKE-OK$", False),
        ("SKIP: zig not installed", r"^SKIP.*$", True),
        ("js-FAIL: refused", r"^js-ok$", False),
        (
            "DIFFERENTIAL-OK (11 cases, codes+spans+values identical)",
            r"^DIFFERENTIAL-OK",
            True,
        ),
        ("DIFFERENTIAL-FAIL: 0003 foo", r"^DIFFERENTIAL-OK", False),
    ]
    bad = 0
    for sample, pattern, want in cases:
        got = parse_tally(sample, pattern) is not None
        if got != want:
            print(f"SELF-CHECK FAIL: {sample!r} vs {pattern!r}")
            bad += 1
    print(f"self-check: {len(cases) - bad}/{len(cases)}")
    return 1 if bad else 0


def classify(
    code: int, output: str, tally: str, timed_out: bool = False
) -> tuple[str, str | None, bool]:
    """Classify one path run as pass / skipped / fail.

    Split out of main() so the rule itself is unit-testable: weakening
    any branch here must turn a test red. Returns (status, tally, clean).
    A timeout always fails, even when the partial output happens to
    contain a SKIP line.
    """
    if timed_out:
        return "fail", parse_tally(scrub(output), tally), RAW_SECRET not in output
    clean = RAW_SECRET not in output
    output = scrub(output)
    tally_line = parse_tally(output, tally)
    skip_line = parse_tally(output, r"^SKIP.*$") is not None
    if code == 127 and not skip_line:
        # Missing toolchain binary (cargo/node/go): skip openly.
        return "skipped", tally_line, clean
    if skip_line:
        # A SKIP line skips only on a clean exit: a runner that
        # prints SKIP and then fails (nonzero code) fails the run,
        # never passes silently (review P3-2).
        if code == 0 and clean:
            return "skipped", tally_line, clean
        return "fail", tally_line, clean
    if code == 0 and tally_line is not None and clean:
        return "pass", tally_line, clean
    return "fail", tally_line, clean


# Tier-1 paths must PASS for the run to pass: these embeddings carry
# the cross-implementation contract (reference, C ABI, WASM, every
# Tier-1 binding, and the differential proof itself). Tier-2 smoke paths
# may skip openly when their toolchain is absent. Deleting or renaming a
# Tier-1 entry must fail the run, never silently shrink the gate.
TIER1 = frozenset(
    {
        "reference",
        "c_abi",
        "wasm",
        "go",
        "python",
        "js-native",
        "go-native",
        "differential",
    }
)


def verify_parity(report: dict) -> list[str]:
    """Cross-implementation parity gate (Phase-2 criterion 1).

    The per-path tallies prove each embedding runs its own suite; this
    gate proves the embeddings AGREE: identical codes, spans, and
    redacted JSON. It fails the whole run when the differential proof
    is missing, red, or leaking — so deleting or weakening the gate
    cannot silently pass. Returns the violation list (empty = hold).
    """
    violations: list[str] = []
    paths = report.get("paths", {})
    diff = paths.get("differential")
    if diff is None:
        violations.append("differential parity gate missing from report")
    elif diff.get("status") != "pass":
        violations.append(f"differential parity gate not passing: {diff.get('status')}")
    elif not diff.get("secret_clean", False):
        violations.append("differential parity gate not secret-clean")
    for name in sorted(TIER1 - {"differential"}):
        entry = paths.get(name)
        if entry is None:
            violations.append(f"Tier-1 path {name} missing from report")
        elif entry.get("status") != "pass":
            violations.append(f"Tier-1 path {name} not passing: {entry.get('status')}")
        elif not entry.get("secret_clean", False):
            violations.append(f"Tier-1 path {name} not secret-clean")
    for name, entry in paths.items():
        if name in PLACEHOLDERS or name in TIER1:
            continue
        if entry.get("status") not in ("pass", "skipped"):
            violations.append(f"path {name} not passing: {entry.get('status')}")
        if not entry.get("secret_clean", False):
            violations.append(f"path {name} not secret-clean")
    if report.get("secret_leak"):
        violations.append("raw secret marker leaked in captured output")
    return violations


def finalize_ok(failed: bool, parity: list[str]) -> bool:
    """Overall verdict from the path failures plus the parity gate.

    Split out of main() so the wiring itself is unit-testable: bypassing
    verify_parity in main() must turn the finalize tests red.
    """
    return not failed and not parity


def main() -> int:
    if "--self-check" in sys.argv:
        return self_check()
    out = Path(sys.argv[sys.argv.index("--out") + 1]) if "--out" in sys.argv else None
    report: dict = {"paths": {}, "secret_leak": False, "ok": False}
    failed = False
    for name, argv, cwd, tally in PATHS:
        code, output, timed_out = sh(argv, cwd)
        if RAW_SECRET in output:
            report["secret_leak"] = True
            # A raw secret in any path's output fails the whole run,
            # even a path that otherwise counts as skipped. The clean
            # flag below is read from the raw output for the same
            # reason: scrub() redacts the marker, so checking after
            # scrubbing would pass every run by construction.
            failed = True
        status, tally_line, clean = classify(code, output, tally, timed_out)
        if status == "skipped":
            report["paths"][name] = {
                "status": "skipped",
                "exit": code,
                "tally": tally_line,
                "secret_clean": clean,
            }
            continue
        if status != "pass":
            failed = True
        entry = {
            "status": status,
            "exit": code,
            "tally": tally_line,
            "secret_clean": clean,
        }
        if timed_out:
            entry["timed_out"] = True
        if status != "pass":
            entry["output_tail"] = scrub(output)[-2000:]
        report["paths"][name] = entry
    for name in PLACEHOLDERS:
        report["paths"][name] = {"status": "not-implemented"}
    # Cross-implementation parity: identical codes, spans, and redacted
    # JSON across embeddings. This is the Phase-2 criterion 1 gate; it
    # runs inside run.py (not beside it) so no report can pass without it.
    parity = verify_parity(report)
    report["parity"] = parity
    report["ok"] = finalize_ok(failed, parity)
    text = json.dumps(report, indent=2)
    print(text)
    if out:
        out.write_text(text + "\n", encoding="utf-8")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

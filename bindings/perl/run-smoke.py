#!/usr/bin/env python3
"""Perl smoke runner for the shared harness (CHECKLIST Phase 2 stretch)."""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def find_sharedlib() -> Path | None:
    if sys.platform == "win32":
        names = ["active_ffi.dll"]
    elif sys.platform == "darwin":
        names = ["libactive_ffi.dylib"]
    else:
        names = ["libactive_ffi.so"]
    for d in (
        ROOT / "target" / "x86_64-unknown-linux-gnu" / "release",
        ROOT / "target" / "release",
        ROOT / "target" / "debug",
    ):
        for name in names:
            cand = d / name
            if cand.is_file():
                return cand
    return None


def main() -> int:
    if shutil.which("perl") is None:
        print("SKIP: perl not installed")
        return 0
    probe = subprocess.run(
        ["perl", "-MFFI::Platypus", "-e", "1"],
        capture_output=True,
        timeout=120,
    )
    if probe.returncode != 0:
        print("SKIP: FFI::Platypus not installed")
        return 0
    lib = find_sharedlib()
    if lib is None:
        print("SKIP: no native shared library built")
        return 0
    p = subprocess.run(
        ["perl", str(ROOT / "bindings" / "perl" / "smoke.pl"), str(lib)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=600,
    )
    print(p.stdout + p.stderr)
    if p.returncode != 0 or "SMOKE-OK" not in (p.stdout + p.stderr):
        return 1
    print("SMOKE-OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

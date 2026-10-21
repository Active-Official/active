#!/usr/bin/env python3
"""Dart smoke runner for the shared harness (CHECKLIST Phase 2 stretch)."""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
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


def run(argv, cwd) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        argv,
        cwd=cwd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=600,
    )


def main() -> int:
    if shutil.which("dart") is None:
        print("SKIP: dart not installed")
        return 0
    lib = find_sharedlib()
    if lib is None:
        print("SKIP: no native shared library built")
        return 0
    tmp = Path(tempfile.mkdtemp(prefix="active-dart-"))
    try:
        for name in ("smoke.dart", "pubspec.yaml"):
            (tmp / name).write_bytes((ROOT / "bindings" / "dart" / name).read_bytes())
        p = run(["dart", "pub", "get"], tmp)
        if p.returncode != 0:
            # A failed dependency fetch is a build failure, never a skip:
            # silently skipping here would mask a broken pubspec.
            print("BUILD FAILED (dart pub get):\n" + (p.stderr or p.stdout)[-2000:])
            return 1
        r = run(["dart", str(tmp / "smoke.dart"), str(lib)], tmp)
        print(r.stdout + r.stderr)
        if r.returncode != 0 or "SMOKE-OK" not in (r.stdout + r.stderr):
            return 1
        print("SMOKE-OK")
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())

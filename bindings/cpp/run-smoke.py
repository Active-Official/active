#!/usr/bin/env python3
"""C++ smoke runner for the shared harness (CHECKLIST 2.003).

Compiles bindings/cpp/smoke.cpp against the in-tree staticlib and runs
it. Prints SMOKE-OK lines; exit 0 on success. Skips when no C++
toolchain or static library is available.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def run(argv: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        argv,
        cwd=cwd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=600,
    )


def find_staticlib() -> Path | None:
    """Search release then debug for the platform static library."""
    if sys.platform == "win32":
        roots = [ROOT / "target" / p for p in ("release", "debug")]
        names = ["active_ffi.lib"]
    else:
        roots = [
            ROOT / "target" / "x86_64-unknown-linux-gnu" / "release",
            ROOT / "target" / "release",
            ROOT / "target" / "debug",
        ]
        names = ["libactive_ffi.a"]
    for root in roots:
        for name in names:
            cand = root / name
            if cand.is_file():
                return cand
    return None


def main() -> int:
    if sys.platform == "win32":
        # MSVC-built objects carry MSVC CRT helpers (e.g. __chkstk)
        # that MinGW ld cannot resolve; proven in the Linux container
        # instead (see bindings/cpp/README.md).
        print("SKIP: MSVC toolchain required on Windows")
        return 0
    if shutil.which("g++") is None and shutil.which("c++") is None:
        print("SKIP: no C++ compiler installed")
        return 0
    cxx = shutil.which("g++") or shutil.which("c++")
    lib = find_staticlib()
    if lib is None:
        print("SKIP: no native static library built (release or debug)")
        return 0
    tmp = Path(tempfile.mkdtemp(prefix="active-cpp-"))
    try:
        return _main_in(tmp, cxx, lib)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def _main_in(tmp: Path, cxx: str, lib: Path) -> int:
    exe = tmp / ("smoke.exe" if sys.platform == "win32" else "smoke")
    b = run(
        [
            cxx,
            "-std=c++17",
            "bindings/cpp/smoke.cpp",
            "-I",
            "active-ffi",
            str(lib),
            "-o",
            str(exe),
        ],
        ROOT,
    )
    if b.returncode != 0:
        print("BUILD FAILED:\n" + (b.stderr or b.stdout)[-2000:])
        return 1
    r = run([str(exe)], tmp)
    print(r.stdout + r.stderr)
    if r.returncode != 0 or "SMOKE-OK" not in (r.stdout + r.stderr):
        return 1
    print("SMOKE-OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Fortran smoke runner for the shared harness (CHECKLIST 4.001)."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def find_staticlib() -> Path | None:
    for base in (
        ROOT / "target" / "x86_64-unknown-linux-gnu" / "release",
        ROOT / "target" / "release",
        ROOT / "target" / "debug",
    ):
        cand = base / "libactive_ffi.a"
        if cand.is_file():
            return cand
    win = ROOT / "target" / "release" / "active_ffi.lib"
    if sys.platform == "win32" and win.is_file():
        return win
    win = ROOT / "target" / "debug" / "active_ffi.lib"
    if sys.platform == "win32" and win.is_file():
        return win
    return None


def run(
    argv: list[str], cwd: Path, extra_env: dict | None = None
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if extra_env:
        env.update(extra_env)
    return subprocess.run(
        argv,
        cwd=cwd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=600,
        env=env,
    )


def main() -> int:
    if shutil.which("gfortran") is None:
        print("SKIP: no Fortran compiler installed")
        return 0
    if sys.platform == "win32":
        # MSVC-built staticlib cannot link under MinGW gfortran
        # (missing __chkstk et al). Proven in the Linux container instead.
        print("SKIP: MSVC toolchain required on Windows")
        return 0
    lib = find_staticlib()
    if lib is None:
        print("SKIP: no native static library built")
        return 0
    # gfortran as a linker needs the GCC runtime on PATH; prefer the
    # compiler's own driver environment, fall back to system PATH.
    tmp = Path(tempfile.mkdtemp(prefix="active-fortran-"))
    try:
        exe = tmp / ("smoke.exe" if sys.platform == "win32" else "smoke")
        # Compile inside the temp dir with absolute paths: gfortran
        # stages temporary module files (*.mod0) in the working
        # directory, and compiling in the repo root both litters the
        # tree and risks temp-file collisions across runs.
        p = run(
            [
                "gfortran",
                str(ROOT / "bindings" / "fortran" / "smoke.f90"),
                "-I",
                str(ROOT / "active-ffi"),
                str(lib),
                "-o",
                str(exe),
            ],
            tmp,
        )
        if p.returncode != 0:
            print("BUILD FAILED:\n" + (p.stderr or p.stdout)[-2000:])
            return 1
        r = run([str(exe)], tmp)
        print(r.stdout + r.stderr)
        if r.returncode != 0 or "SMOKE-OK" not in (r.stdout + r.stderr):
            return 1
        print("SMOKE-OK")
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())

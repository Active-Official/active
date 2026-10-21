#!/usr/bin/env python3
"""V smoke runner for the shared harness (CHECKLIST Phase 2 stretch)."""

from __future__ import annotations

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
    return None


def run(argv, cwd) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        argv,
        cwd=cwd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=900,
    )


def main() -> int:
    if shutil.which("v") is None:
        print("SKIP: v not installed")
        return 0
    if sys.platform == "win32":
        print("SKIP: MSVC toolchain required on Windows")
        return 0
    lib = find_staticlib()
    if lib is None:
        print("SKIP: no native static library built")
        return 0
    tmp = Path(tempfile.mkdtemp(prefix="active-v-"))
    try:
        src = tmp / "smoke.v"
        src.write_bytes((ROOT / "bindings" / "vlang" / "smoke.v").read_bytes())
        exe = tmp / "smoke"
        p = run(
            [
                "v",
                "-cflags",
                f"-I{ROOT / 'active-ffi'}",
                "-ldflags",
                str(lib),
                "-ldflags",
                "-lm",
                "-ldflags",
                "-ldl",
                "-ldflags",
                "-lpthread",
                "-o",
                str(exe),
                str(src),
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

#!/usr/bin/env python3
"""LuaJIT smoke runner for the shared harness (CHECKLIST 2.016).

Runs bindings/lua/smoke.lua against the in-tree shared library.
Prints SMOKE-OK lines; exit 0 on success. Skips when LuaJIT or the
Linux shared library is absent.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def find_sharedlib() -> Path | None:
    """Search release then debug for the platform shared library."""
    if sys.platform == "win32":
        names = ("active_ffi.dll",)
    elif sys.platform == "darwin":
        names = ("libactive_ffi.dylib",)
    else:
        names = ("libactive_ffi.so",)
    roots = [
        ROOT / "target" / "x86_64-unknown-linux-gnu" / "release",
        ROOT / "target" / "release",
        ROOT / "target" / "debug",
    ]
    for root in roots:
        for name in names:
            cand = root / name
            if cand.is_file():
                return cand
    return None


def main() -> int:
    if shutil.which("luajit") is None:
        print("SKIP: luajit not installed")
        return 0
    lib = find_sharedlib()
    if lib is None:
        print("SKIP: no native shared library built")
        return 0
    p = subprocess.run(
        ["luajit", "bindings/lua/smoke.lua", str(lib)],
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

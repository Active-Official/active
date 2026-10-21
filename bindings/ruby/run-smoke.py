#!/usr/bin/env python3
"""Ruby smoke runner for the shared harness (CHECKLIST 2.005)."""

from __future__ import annotations

import os
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
    if shutil.which("ruby") is None:
        print("SKIP: ruby not installed")
        return 0
    lib = find_sharedlib()
    if lib is None:
        print("SKIP: no native shared library built")
        return 0
    env = dict(os.environ)
    env.pop("RUBYOPT", None)
    p = subprocess.run(
        ["ruby", str(ROOT / "bindings" / "ruby" / "smoke.rb"), str(lib)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=600,
        env=env,
    )
    print(p.stdout + p.stderr)
    if p.returncode != 0 or "SMOKE-OK" not in (p.stdout + p.stderr):
        return 1
    print("SMOKE-OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

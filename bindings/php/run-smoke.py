#!/usr/bin/env python3
"""PHP smoke runner for the shared harness (CHECKLIST 2.006)."""

from __future__ import annotations

import re
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
    if shutil.which("php") is None:
        print("SKIP: php not installed")
        return 0
    lib = find_sharedlib()
    if lib is None:
        print("SKIP: no native shared library built")
        return 0
    p = subprocess.run(
        [
            "php",
            "-d",
            "ffi.enable=1",
            str(ROOT / "bindings" / "php" / "smoke.php"),
            str(lib),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=600,
    )
    print(p.stdout + p.stderr)
    out = p.stdout + p.stderr
    if p.returncode == 0 and "SMOKE-OK" in out:
        print("SMOKE-OK")
        return 0
    if p.returncode == 0 and re.search(r"^SKIP", out, re.MULTILINE):
        # The smoke itself skipped (e.g. no ext-ffi on this host):
        # propagate the skip instead of failing on the missing tally.
        return 0
    return 1


if __name__ == "__main__":
    raise SystemExit(main())

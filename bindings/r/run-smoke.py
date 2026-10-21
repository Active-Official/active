#!/usr/bin/env python3
"""R smoke runner for the shared harness (CHECKLIST Phase 2 stretch)."""

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
    return None


def run(argv, cwd, extra_env=None) -> subprocess.CompletedProcess[str]:
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
    if shutil.which("Rscript") is None:
        print("SKIP: Rscript not installed")
        return 0
    if sys.platform == "win32":
        print("SKIP: MSVC toolchain required on Windows")
        return 0
    lib = find_staticlib()
    if lib is None:
        print("SKIP: no native static library built")
        return 0
    tmp = Path(tempfile.mkdtemp(prefix="active-r-"))
    try:
        for name in ("smoke.R", "r_wrap.c"):
            (tmp / name).write_bytes((ROOT / "bindings" / "r" / name).read_bytes())
        p = run(
            ["R", "CMD", "SHLIB", "r_wrap.c"],
            tmp,
            extra_env={
                "PKG_LIBS": f"{lib} -lpthread -ldl -lm",
                "PKG_CPPFLAGS": f"-I{ROOT / 'active-ffi'}",
            },
        )
        if p.returncode != 0:
            print("BUILD FAILED:\n" + (p.stderr or p.stdout)[-2000:])
            return 1
        sos = sorted(tmp.glob("*.so"))
        if not sos:
            print("BUILD FAILED: no wrapper .so produced")
            return 1
        r = run(["Rscript", str(tmp / "smoke.R"), str(sos[0])], tmp)
        print(r.stdout + r.stderr)
        if r.returncode != 0 or "SMOKE-OK" not in (r.stdout + r.stderr):
            return 1
        print("SMOKE-OK")
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())

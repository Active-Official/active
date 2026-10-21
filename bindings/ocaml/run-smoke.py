#!/usr/bin/env python3
"""OCaml smoke runner for the shared harness (CHECKLIST Phase 2 stretch)."""

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
        timeout=600,
    )


def main() -> int:
    if shutil.which("ocamlopt") is None:
        print("SKIP: ocamlopt not installed")
        return 0
    if sys.platform == "win32":
        print("SKIP: MSVC toolchain required on Windows")
        return 0
    lib = find_staticlib()
    if lib is None:
        print("SKIP: no native static library built")
        return 0
    tmp = Path(tempfile.mkdtemp(prefix="active-ocaml-"))
    try:
        for name in ("smoke.ml", "ocaml_stubs.c"):
            (tmp / name).write_bytes((ROOT / "bindings" / "ocaml" / name).read_bytes())
        exe = tmp / "smoke"
        p = run(
            [
                "ocamlopt",
                "-o",
                str(exe),
                "-ccopt",
                f"-I{ROOT / 'active-ffi'}",
                "ocaml_stubs.c",
                "smoke.ml",
                str(lib),
                "-cclib",
                "-lm",
                "-cclib",
                "-ldl",
                "-cclib",
                "-lpthread",
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

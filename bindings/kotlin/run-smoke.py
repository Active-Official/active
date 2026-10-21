#!/usr/bin/env python3
"""Kotlin smoke runner for the shared harness (CHECKLIST Phase 2 stretch)."""

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
        timeout=900,
        env=env,
    )


def java_home() -> Path | None:
    javac = shutil.which("javac")
    if javac is None:
        return None
    real = Path(javac).resolve()
    # .../bin/javac -> home is two levels up.
    home = real.parent.parent
    if (home / "include" / "jni.h").is_file() and (
        home / "include" / "linux" / "jni_md.h"
    ).is_file():
        return home
    return None


def main() -> int:
    if shutil.which("kotlinc") is None or shutil.which("java") is None:
        print("SKIP: kotlinc or java not installed")
        return 0
    if shutil.which("gcc") is None:
        print("SKIP: gcc not installed")
        return 0
    home = java_home()
    if home is None:
        print("SKIP: JDK with JNI headers not installed")
        return 0
    if sys.platform == "win32":
        print("SKIP: MSVC toolchain required on Windows")
        return 0
    lib = find_staticlib()
    if lib is None:
        print("SKIP: no native static library built")
        return 0
    tmp = Path(tempfile.mkdtemp(prefix="active-kotlin-"))
    try:
        for name in ("KActive.kt", "active_kni.c"):
            (tmp / name).write_bytes((ROOT / "bindings" / "kotlin" / name).read_bytes())
        p = run(
            [
                "gcc",
                "-shared",
                "-fPIC",
                f"-I{home / 'include'}",
                f"-I{home / 'include' / 'linux'}",
                f"-I{ROOT / 'active-ffi'}",
                "active_kni.c",
                "-o",
                "libactive_kni.so",
                str(lib),
            ],
            tmp,
        )
        if p.returncode != 0:
            print("BUILD FAILED (shim):\n" + (p.stderr or p.stdout)[-2000:])
            return 1
        p = run(["kotlinc", "KActive.kt", "-include-runtime", "-d", "kactive.jar"], tmp)
        if p.returncode != 0:
            print("BUILD FAILED (kotlinc):\n" + (p.stderr or p.stdout)[-2000:])
            return 1
        r = run(["java", f"-Djava.library.path={tmp}", "-jar", "kactive.jar"], tmp)
        print(r.stdout + r.stderr)
        if r.returncode != 0 or "SMOKE-OK" not in (r.stdout + r.stderr):
            return 1
        print("SMOKE-OK")
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())

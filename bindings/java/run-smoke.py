#!/usr/bin/env python3
"""Java smoke runner for the shared harness (CHECKLIST 2.001).

Compiles Active.java + SmokeTest.java, generates JNI headers, builds
libactive_jni.so against the in-tree staticlib, and runs the smoke test.
Prints SMOKE-OK lines; exit 0 on success. Skips when no JDK or C
compiler is available.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent


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


def find_staticlib() -> Path | None:
    """Search release then debug for the platform static library."""
    candidates = []
    if sys.platform == "win32":
        candidates = [
            ROOT / "target" / p / "active_ffi.lib" for p in ("release", "debug")
        ]
    else:
        candidates = [
            ROOT
            / "target"
            / "x86_64-unknown-linux-gnu"
            / "release"
            / "libactive_ffi.a",
            ROOT / "target" / "debug" / "libactive_ffi.a",
        ]
    for c in candidates:
        if c.is_file():
            return c
    return None


def main() -> int:
    javac = shutil.which("javac")
    java = shutil.which("java")
    gcc = shutil.which("gcc") or shutil.which("cc")
    if javac is None or java is None or gcc is None:
        print("SKIP: JDK (javac+java) or C compiler not installed")
        return 0
    java_home = Path(javac).resolve().parent.parent
    inc = java_home / "include"
    plat = inc / (
        "win32"
        if sys.platform == "win32"
        else "darwin"
        if sys.platform == "darwin"
        else "linux"
    )
    lib = find_staticlib()
    if lib is None:
        print("SKIP: no native static library built (release or debug)")
        return 0
    tmp = Path(tempfile.mkdtemp(prefix="active-java-"))
    try:
        return _main_in(tmp, java_home, inc, plat, lib, gcc)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def _main_in(
    tmp: Path, java_home: Path, inc: Path, plat: Path, lib: Path, gcc: str
) -> int:
    for step in (
        [
            "javac",
            "-d",
            str(tmp),
            str(HERE / "Active.java"),
            str(HERE / "SmokeTest.java"),
        ],
        ["javac", "-h", str(tmp), "-d", str(tmp), str(HERE / "Active.java")],
        [
            gcc,
            "-shared",
            "-fPIC",
            f"-I{inc}",
            f"-I{plat}",
            "-I",
            str(ROOT / "active-ffi"),
            str(HERE / "active_jni.c"),
            str(lib),
            "-o",
            str(
                tmp
                / ("active_jni.dll" if sys.platform == "win32" else "libactive_jni.so")
            ),
        ],
    ):
        p = run(step, ROOT)
        if p.returncode != 0:
            print("BUILD FAILED:\n" + (p.stderr or p.stdout)[-2000:])
            return 1
    r = run(
        ["java", "-Djava.library.path=" + str(tmp), "-cp", str(tmp), "SmokeTest"], tmp
    )
    print(r.stdout + r.stderr)
    if r.returncode != 0 or "SMOKE-OK" not in (r.stdout + r.stderr):
        return 1
    print("SMOKE-OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

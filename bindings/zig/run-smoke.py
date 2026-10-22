#!/usr/bin/env python3
"""Zig smoke runner for the shared harness (CHECKLIST 2.003).

Builds bindings/zig/smoke.zig against the in-tree active-ffi staticlib
and runs it. Prints SMOKE-OK on success (the harness tally).
Assumes `cargo build -p active-ffi` already ran (CI release job does).
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
import time
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


def build_with_retry(cmd: list[str]) -> subprocess.CompletedProcess[str]:
    # The zig Windows toolchain intermittently fails to stage the C
    # import (FileNotFound on a committed header) under runner load;
    # identical code passes on retry and in the Linux container.
    # Retry the toolchain invocation (max 3 attempts, backoff) while
    # keeping the gate strict: deterministic breakage fails every
    # attempt identically and still fails the run. Every attempt is
    # logged so retries never hide silently.
    last: subprocess.CompletedProcess[str] | None = None
    for attempt in (1, 2, 3):
        last = run(cmd, ROOT)
        if last.returncode == 0:
            return last
        print(f"zig build attempt {attempt}/3 failed; retrying...")
        print((last.stderr or last.stdout)[-500:])
        time.sleep(5 * attempt)
    assert last is not None
    return last


def main() -> int:
    if shutil.which("zig") is None:
        print("SKIP: zig not installed")
        return 0
    # Absolute include path: @cImport resolution must not depend on how
    # the driver was invoked. Fail fast with the exact missing path
    # instead of a bare translator FileNotFound.
    header = ROOT / "active-ffi" / "active.h"
    if not header.is_file():
        print(f"BUILD FAILED: missing header: {header}")
        return 1
    ver = run(["zig", "version"], ROOT)
    print(f"zig toolchain: {(ver.stdout or '').strip()}")
    tmp = Path(tempfile.mkdtemp(prefix="active-zig-"))
    exe = tmp / ("smoke.exe" if sys.platform == "win32" else "smoke")
    # Windows links the import library (proven green) with the DLL staged
    # beside the exe. Linux/macOS link the static archive directly so no
    # runtime loader path is needed.
    link: list[str] = []
    if sys.platform == "win32":
        link = ["-L", str(ROOT / "target" / "debug"), "-l", "active_ffi"]
    else:
        staticlib = ROOT / "target" / "debug" / "libactive_ffi.a"
        if not staticlib.is_file():
            print(f"BUILD FAILED: missing static archive: {staticlib}")
            return 1
        # The Rust staticlib needs the unwinder at link time.
        link = [str(staticlib), "-lunwind"]
    b = build_with_retry(
        [
            "zig",
            "build-exe",
            "bindings/zig/smoke.zig",
            "-I",
            str(ROOT / "active-ffi"),
            *link,
            "-lc",
            "-femit-bin=" + str(exe),
        ]
    )
    if b.returncode != 0:
        print("BUILD FAILED:\n" + (b.stderr or b.stdout)[-2000:])
        return 1
    if sys.platform == "win32":
        dll = ROOT / "target" / "debug" / "active_ffi.dll"
        if dll.is_file():
            shutil.copy(dll, tmp / "active_ffi.dll")
    r = run([str(exe)], tmp)
    print(r.stdout + r.stderr)
    if r.returncode != 0 or "SMOKE-OK" not in (r.stdout + r.stderr):
        return 1
    print("SMOKE-OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

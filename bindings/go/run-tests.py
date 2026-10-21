#!/usr/bin/env python3
"""Go native binding tests via the external active-go repo (CHECKLIST 2.001).

The canonical Go binding lives in its own repo
(Active-Official/active-go, wazero-based, zero cgo) rather than as an
in-tree duplicate. This runner shallow-clones that repo into a temp dir
and runs its full test suite, which exercises the shared conformance
manifest. Prints TEST-OK on a green run; SKIP when git/go is missing or
the clone fails (network-gated, never a hard failure).
"""

from __future__ import annotations

import shutil
import subprocess
import tempfile
from pathlib import Path

REPO = "https://github.com/Active-Official/active-go"


def run(argv, cwd):
    return subprocess.run(
        argv,
        cwd=cwd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=600,
    )


def main():
    if shutil.which("git") is None or shutil.which("go") is None:
        print("SKIP: git or Go toolchain not installed")
        return 0
    tmp = Path(tempfile.mkdtemp(prefix="active-go-native-"))
    try:
        p = run(
            ["git", "clone", "--quiet", "--depth", "1", REPO, str(tmp / "active-go")],
            Path.cwd(),
        )
        if p.returncode != 0:
            print("SKIP: cannot clone active-go (offline?)")
            return 0
        t = run(["go", "test", "./..."], tmp / "active-go")
        combined = t.stdout + t.stderr
        print(combined[-2000:] if len(combined) > 2000 else combined)
        if t.returncode != 0 or "FAIL" in combined:
            return 1
        if "no test files" in combined and "ok " not in combined:
            return 1
        print("TEST-OK")
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())

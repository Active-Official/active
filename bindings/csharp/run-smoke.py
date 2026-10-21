#!/usr/bin/env python3
"""C# smoke runner for the shared harness (CHECKLIST 2.002).

Builds bindings/csharp/ActiveSmoke (no NuGet packages: offline-safe),
stages active_ffi.dll beside the output, runs it, and checks SMOKE-OK.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PROJ = ROOT / "bindings" / "csharp" / "ActiveSmoke"


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


def main() -> int:
    if shutil.which("dotnet") is None:
        print("SKIP: dotnet not installed")
        return 0
    b = run(["dotnet", "build", "-v", "q", "--nologo"], PROJ)
    if b.returncode != 0:
        print("BUILD FAILED:\n" + (b.stderr or b.stdout)[-2000:])
        return 1
    dll = ROOT / "target" / "debug" / "active_ffi.dll"
    outdir = PROJ / "bin" / "Debug"
    tfm = sorted([d for d in outdir.iterdir() if d.is_dir()], key=lambda d: d.name)
    if dll.is_file() and tfm:
        shutil.copy(dll, tfm[0] / "active_ffi.dll")
    r = run(["dotnet", "run", "--project", str(PROJ), "--no-build"], ROOT)
    print(r.stdout + r.stderr)
    if r.returncode != 0 or "SMOKE-OK" not in (r.stdout + r.stderr):
        return 1
    print("SMOKE-OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Elixir smoke runner for the shared harness (CHECKLIST Phase 2 stretch)."""

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


def run(argv, cwd, extra_env=None) -> subprocess.CompletedProcess[str]:
    import os

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
    if shutil.which("elixir") is None or shutil.which("gcc") is None:
        print("SKIP: elixir or gcc not installed")
        return 0
    if sys.platform == "win32":
        print("SKIP: MSVC toolchain required on Windows")
        return 0
    lib = find_staticlib()
    if lib is None:
        print("SKIP: no native static library built")
        return 0
    erts = run(["elixir", "-e", "IO.puts :code.lib_dir(:erts)"], ROOT)
    cands = [Path(erts.stdout.strip()) / "include"]
    # Debian/Ubuntu erlang-dev also ships headers outside the erts app dir.
    cands += [Path("/usr/lib/erlang/usr/include"), Path("/usr/include")]
    erts_inc = next((d for d in cands if (d / "erl_nif.h").is_file()), None)
    if erts_inc is None:
        print("SKIP: Erlang NIF headers not installed (need erlang-dev)")
        return 0
    tmp = Path(tempfile.mkdtemp(prefix="active-elixir-"))
    try:
        for name in ("smoke.exs", "active_nif.c"):
            (tmp / name).write_bytes((ROOT / "bindings" / "elixir" / name).read_bytes())
        p = run(
            [
                "gcc",
                "-shared",
                "-fPIC",
                "-Wall",
                f"-I{erts_inc}",
                f"-I{ROOT / 'active-ffi'}",
                "active_nif.c",
                "-o",
                "active_nif.so",
                str(lib),
            ],
            tmp,
        )
        if p.returncode != 0:
            print("BUILD FAILED:\n" + (p.stderr or p.stdout)[-2000:])
            return 1
        r = run(
            ["elixir", str(tmp / "smoke.exs")],
            tmp,
            extra_env={"ACTIVE_NIF_SO": str(tmp / "active_nif")},
        )
        print(r.stdout + r.stderr)
        if r.returncode != 0 or "SMOKE-OK" not in (r.stdout + r.stderr):
            return 1
        print("SMOKE-OK")
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())

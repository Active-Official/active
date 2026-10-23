#!/usr/bin/env python3
"""Neovim plugin smoke (TOOLING EDITOR.006).

Checks the Lua layer parses and exposes the documented commands. Uses
`luac -p` when available, then `nvim --headless` when available; with
neither toolchain it SKIPs openly (never silently passes).
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
LUA = ROOT / "editors" / "neovim" / "lua" / "active.lua"
COMMANDS = [
    "ActiveCheck",
    "ActiveGraph",
    "ActiveRepair",
    "ActiveCapability",
    "ActiveMode",
]


def main() -> int:
    text = LUA.read_text(encoding="utf-8")
    for command in COMMANDS:
        if command not in text:
            print(f"NEOVIM-SMOKE: missing command {command}")
            return 1
    if shutil.which("luac"):
        proc = subprocess.run(["luac", "-p", str(LUA)], capture_output=True, text=True)
        if proc.returncode != 0:
            print(f"NEOVIM-SMOKE: luac failed: {proc.stderr[:400]}")
            return 1
        print("NEOVIM-SMOKE: OK (luac parse)")
        return 0
    if shutil.which("nvim"):
        proc = subprocess.run(
            [
                "nvim",
                "--headless",
                "-u",
                "NONE",
                "-c",
                f"lua assert(loadfile([[{LUA.as_posix()}]]))",
                "-c",
                "qa",
            ],
            capture_output=True,
            text=True,
            timeout=60,
        )
        if proc.returncode != 0:
            print(f"NEOVIM-SMOKE: nvim load failed: {proc.stderr[:400]}")
            return 1
        print("NEOVIM-SMOKE: OK (nvim loadfile)")
        return 0
    print("SKIP: neither luac nor nvim installed")
    return 0


if __name__ == "__main__":
    sys.exit(main())

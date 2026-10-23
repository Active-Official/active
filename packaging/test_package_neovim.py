#!/usr/bin/env python3
"""Determinism test for the Neovim package (TOOLING FINAL.003).

Builds the archive twice into a temp dir and asserts identical bytes.
Exit nonzero on drift.
"""

from __future__ import annotations

import hashlib
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def workspace_version() -> str:
    for line in (ROOT / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        if line.startswith("version = "):
            return line.split('"')[1]
    raise SystemExit("workspace version not found")


def build(out: Path) -> Path:
    subprocess.run(
        [
            sys.executable,
            str(ROOT / "packaging" / "package-neovim.py"),
            "--out",
            str(out),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    archives = list(out.glob("active-neovim-*.tar.gz"))
    if len(archives) != 1:
        raise SystemExit(f"expected one archive, found {archives}")
    return archives[0]


def main() -> int:
    with (
        tempfile.TemporaryDirectory(prefix="active-nvim-") as first,
        tempfile.TemporaryDirectory(prefix="active-nvim-") as second,
    ):
        a = build(Path(first))
        b = build(Path(second))
        expected = f"active-neovim-{workspace_version()}.tar.gz"
        if a.name != expected:
            print(f"NEOVIM-PACKAGE: WRONG NAME {a.name} != {expected}")
            return 1
        ha = hashlib.sha256(a.read_bytes()).hexdigest()
        hb = hashlib.sha256(b.read_bytes()).hexdigest()
        if ha != hb:
            print(f"NE0VIM-PACKAGE: MISMATCH {ha} != {hb}")
            return 1
        print(f"NEOVIM-PACKAGE: OK sha256:{ha} size={a.stat().st_size}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

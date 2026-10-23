#!/usr/bin/env python3
"""Package the Neovim plugin as a deterministic tar.gz (TOOLING FINAL.003).

Usage: python packaging/package-neovim.py [--out dist]
Writes active-neovim-<version>.tar.gz containing active-neovim/ with the
runtimepath tree. Sorted entries and fixed metadata keep the archive
byte-stable for the same inputs.
"""

from __future__ import annotations

import argparse
import gzip
import io
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PLUGIN = ROOT / "editors" / "neovim"


def workspace_version() -> str:
    for line in (ROOT / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        if line.startswith("version = "):
            return line.split('"')[1]
    raise SystemExit("workspace version not found")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default="dist")
    args = parser.parse_args()
    out_dir = ROOT / args.out
    out_dir.mkdir(parents=True, exist_ok=True)
    version = workspace_version()
    target = out_dir / f"active-neovim-{version}.tar.gz"

    files = sorted(
        path
        for path in PLUGIN.rglob("*")
        if path.is_file() and "__pycache__" not in path.parts
    )
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as tar:
        for path in files:
            data = path.read_bytes()
            info = tarfile.TarInfo(
                f"active-neovim/{path.relative_to(PLUGIN).as_posix()}"
            )
            info.size = len(data)
            info.mtime = 0
            info.mode = 0o644
            info.uid = 0
            info.gid = 0
            info.uname = "root"
            info.gname = "root"
            tar.addfile(info, io.BytesIO(data))
    with open(target, "wb") as raw:
        with gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as gz:
            gz.write(buffer.getvalue())

    print(f"packaged {target} ({len(files)} files, {target.stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

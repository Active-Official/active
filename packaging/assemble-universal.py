#!/usr/bin/env python3
"""Assemble the universal manual-wiring artifact (CHECKLIST D.006).

Collects the frozen C ABI header, the release native libraries, and
the zero-import WASM module into one versioned tarball for hosts
without a Tier-1 binding:

  active-universal-<version>/
    include/active.h          frozen C ABI v1 header (docs/FFI.md)
    lib/                      static + shared libraries per platform build
    bin/active_wasm.wasm      zero-import module + hand-written shims
    README.txt                wiring pointers (FFI docs, install doc)

Usage: python packaging/assemble-universal.py --version 1.4.0 [--out-dir dist-universal]
Prints the tarball path plus sha256. Libraries come from the local
release builds (cargo build -p active-ffi --release and the wasm
target); the script fails closed when any input is missing.

Maintainer-run per release: the tarball is multi-MB evidence, not
committed. The release job regenerates SBOMs the same way (see
packaging/README.md); this tarball is attached by the maintainer.
"""

from __future__ import annotations

import argparse
import hashlib
import shutil
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def stage(src: Path, dst: Path) -> None:
    if not src.is_file():
        raise SystemExit(f"FAIL: missing input {src} (build it first)")
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--out-dir", default="dist-universal")
    ns = parser.parse_args()

    import re

    if not re.fullmatch(r"\d+\.\d+\.\d+", ns.version):
        print(f"FAIL: refusing suspicious version: {ns.version!r}")
        return 1
    # The tarball stamp must equal the workspace release line — a
    # mistamped manual artifact would misroute every consumer.
    workspace = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    anchor = ns.version
    if f'version = "{anchor}"' not in workspace:
        print(f"FAIL: {ns.version} is not the workspace [workspace.package] version")
        return 1

    name = f"active-universal-{ns.version}"
    if Path(ns.out_dir).is_absolute():
        print(f"FAIL: --out-dir must be a repo-relative path: {ns.out_dir!r}")
        return 1
    out_dir = (ROOT / ns.out_dir).resolve()
    if ROOT.resolve() not in out_dir.parents:
        print(f"FAIL: --out-dir escapes the repo: {ns.out_dir!r}")
        return 1
    if out_dir.exists():
        shutil.rmtree(out_dir)
    root = out_dir / name
    target = ROOT / "target"

    stage(ROOT / "active-ffi" / "active.h", root / "include" / "active.h")
    lib = root / "lib"
    lib.mkdir(parents=True, exist_ok=True)
    for pattern in (
        "active_ffi.lib",
        "active_ffi.dll",
        "libactive_ffi.so",
        "libactive_ffi.dylib",
        "libactive_ffi.a",
    ):
        for cand in (target / "release").glob(pattern):
            stage(cand, lib / cand.name)
    if not any(lib.iterdir()):
        raise SystemExit("FAIL: no native libraries found under target/release")
    stage(
        target / "wasm32-unknown-unknown" / "release" / "active_wasm.wasm",
        root / "bin" / "active_wasm.wasm",
    )
    stage(
        ROOT / "active-wasm" / "js" / "active-shim.js",
        root / "bin" / "active-shim.js",
    )
    (root / "README.txt").write_text(
        "Active universal wiring artifact (frozen C ABI v1).\n"
        "include/active.h : opaque handles, Allocator Model A (docs/FFI.md).\n"
        "lib/            : link the static lib or ship the shared lib.\n"
        "bin/            : active_wasm.wasm plus the hand-written JS shim\n"
        "                  (active-shim.js) for WASM hosts.\n"
        "Ownership, versions, and install routes: docs/install.md and\n"
        "docs/version-and-abi-policy.md.\n",
        encoding="utf-8",
    )
    tarball = out_dir / f"{name}.tar.gz"

    def _normalize(ti: tarfile.TarInfo) -> tarfile.TarInfo:
        ti.uid = ti.gid = 0
        ti.uname = ti.gname = ""
        ti.mtime = 0
        return ti

    # Deterministic bytes: normalized tar members plus a gzip layer
    # with a fixed mtime (tarfile's w:gz stamps wall time).
    import gzip

    raw_tar = out_dir / f"{name}.tar"
    with tarfile.open(raw_tar, "w:", format=tarfile.PAX_FORMAT) as tf:
        tf.add(root, arcname=name, filter=_normalize)
    with open(raw_tar, "rb") as f_in, open(tarball, "wb") as f_out:
        with gzip.GzipFile(
            filename="", mode="wb", compresslevel=9, mtime=0, fileobj=f_out
        ) as gz:
            shutil.copyfileobj(f_in, gz)
    raw_tar.unlink()
    digest = hashlib.sha256(tarball.read_bytes()).hexdigest()
    print(f"UNIVERSAL: {tarball} ({tarball.stat().st_size} bytes, sha256 {digest})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Fetch the release WASM module for distributable bundles.

Downloads ``active_wasm.wasm`` for a workspace version from the
Active-Official/active GitHub release, verifies it against the
release ``SHA256SUMS``, and writes it into each given directory
(e.g. ``bindings/python``, ``bindings/js``) where the Tier-1
bindings pick it up as the bundled module.

Usage: python packaging/fetch-wasm.py --version 1.4.0 --into bindings/python bindings/js
"""

from __future__ import annotations

import argparse
import hashlib
import re
import shutil
import tempfile
import urllib.request
from pathlib import Path

ORG_RELEASE = (
    "https://github.com/Active-Official/active/releases/download/v{version}/{asset}"
)


def fetch_text(url: str) -> str:
    req = urllib.request.Request(url, headers={"User-Agent": "active-fetch-wasm"})
    try:
        with urllib.request.urlopen(req, timeout=120) as r:
            return r.read().decode("utf-8")
    except Exception as e:
        raise SystemExit(f"FAIL: cannot fetch {url}: {e}")


def fetch_bytes(url: str) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": "active-fetch-wasm"})
    try:
        with urllib.request.urlopen(req, timeout=600) as r:
            return r.read()
    except Exception as e:
        raise SystemExit(f"FAIL: cannot fetch {url}: {e}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--version", required=True, help="workspace version, e.g. 1.4.0"
    )
    parser.add_argument("--into", nargs="+", required=True, help="target directories")
    ns = parser.parse_args()
    if not re.fullmatch(r"\d+\.\d+\.\d+", ns.version):
        print(f"FAIL: refusing suspicious version: {ns.version!r}")
        return 1

    sums_url = ORG_RELEASE.format(version=ns.version, asset="SHA256SUMS")
    sums = fetch_text(sums_url)
    want = None
    for line in sums.splitlines():
        digest, _, name = line.strip().partition(" ")
        name = name.strip().lstrip("*")
        if name == "active_wasm.wasm":
            want = digest.strip()
    if not want or not re.fullmatch(r"[0-9a-f]{64}", want):
        print(
            "FAIL: active_wasm.wasm missing (or malformed digest) in release SHA256SUMS"
        )
        return 1

    blob = fetch_bytes(ORG_RELEASE.format(version=ns.version, asset="active_wasm.wasm"))
    got = hashlib.sha256(blob).hexdigest()
    if got != want:
        print(f"FAIL: checksum mismatch: got {got}, want {want}")
        return 1
    for target in ns.into:
        dest_dir = Path(target)
        dest_dir.mkdir(parents=True, exist_ok=True)
        dest = dest_dir / "active_wasm.wasm"
        with tempfile.NamedTemporaryFile(dir=str(dest_dir), delete=False) as tmp:
            tmp.write(blob)
            tmp_path = Path(tmp.name)
        shutil.move(str(tmp_path), str(dest))
        print(f"wrote {dest} ({len(blob)} bytes, sha256 {got[:16]}...)")
    print("FETCH-WASM: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

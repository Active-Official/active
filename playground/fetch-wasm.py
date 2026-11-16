#!/usr/bin/env python3
"""Stage the playground's engine (LANG-G-023).

`playground/index.html` loads `playground/active_wasm.wasm`. The file is a
generated artefact (gitignored, never committed): this script copies the
release build next to the page, or downloads the verified release asset
with `python packaging/fetch-wasm.py --version <v> --into playground`.

Usage:
  cargo build -p active-wasm --target wasm32-unknown-unknown --release
  python playground/fetch-wasm.py            # copy the local build
  python playground/fetch-wasm.py --check    # verify the staged copy

Exit 0 on success; exit 2 when the release build is absent (the message
names the build command); exit 1 when --check finds a missing or stale
artefact.
"""

from __future__ import annotations

import argparse
import hashlib
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "target" / "wasm32-unknown-unknown" / "release" / "active_wasm.wasm"
DEST = Path(__file__).resolve().parent / "active_wasm.wasm"
BUILD = "cargo build -p active-wasm --target wasm32-unknown-unknown --release"


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify the staged copy instead of writing it",
    )
    ns = parser.parse_args()
    if not SOURCE.is_file():
        print(
            f"release wasm not found: {SOURCE}\nbuild it first: {BUILD}",
            file=sys.stderr,
        )
        return 2
    if ns.check:
        if not DEST.is_file():
            print(
                f"FAIL: {DEST} is missing; run this script without --check",
                file=sys.stderr,
            )
            return 1
        if digest(DEST) != digest(SOURCE):
            print(
                f"FAIL: {DEST} is stale; rerun this script to restage it",
                file=sys.stderr,
            )
            return 1
        print(f"PLAYGROUND-WASM: OK ({DEST.name}, {DEST.stat().st_size} bytes)")
        return 0
    shutil.copyfile(SOURCE, DEST)
    if digest(DEST) != digest(SOURCE):
        print(f"FAIL: copy to {DEST} did not match the source", file=sys.stderr)
        return 1
    print(f"wrote {DEST} ({DEST.stat().st_size} bytes, sha256 {digest(DEST)[:16]}...)")
    print("PLAYGROUND-WASM: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

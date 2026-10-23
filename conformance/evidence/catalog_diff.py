#!/usr/bin/env python3
"""Error-code catalog diff (evidence artifact).

Every diagnostic code emitted by shipped source must appear in the public
catalog (`docs/diagnostics.md`). This script extracts code literals from the
source trees, extracts catalogued codes from the document, and reports any
emitted-but-undocumented code as a failure.

Usage: python conformance/evidence/catalog_diff.py
Exit 0 only when every emitted code is catalogued. Prints a CATALOG line.
"""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SRC_DIRS = [
    ROOT / "active-core" / "src",
    ROOT / "active-cli" / "src",
    ROOT / "active-lsp" / "src",
    ROOT / "active-ffi" / "src",
    ROOT / "active-wasm" / "src",
]
CATALOG = ROOT / "docs" / "diagnostics.md"

CODE = re.compile(r"\b([ESTXHBCWL][0-9]{3})\b")
# Only string literals count as emissions: a code in a comment does not ship.
EMITTED = re.compile(r'"([ESTXHBCWL][0-9]{3})"')


def main() -> int:
    emitted: dict[str, list[str]] = {}
    for base in SRC_DIRS:
        for path in sorted(base.rglob("*.rs")):
            text = path.read_text(encoding="utf-8", errors="replace")
            for code in EMITTED.findall(text):
                emitted.setdefault(code, []).append(
                    str(path.relative_to(ROOT)).replace("\\", "/")
                )

    catalog_text = CATALOG.read_text(encoding="utf-8", errors="replace")
    catalogued = set(CODE.findall(catalog_text))

    missing = sorted(set(emitted) - catalogued)
    if missing:
        for code in missing:
            files = ", ".join(sorted(set(emitted[code]))[:3])
            print(f"CATALOG-MISSING {code} emitted in {files}")
        print(
            f"CATALOG: {len(emitted)} emitted, {len(catalogued)} catalogued, "
            f"{len(missing)} missing"
        )
        return 1

    print(f"CATALOG: {len(emitted)} emitted, {len(catalogued)} catalogued, 0 missing")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Dead-reference gate for repository docs (LANG-G-020).

`AGENTS.md` sent every agent to `docs/Agent-Handbook.md`, a gitignored
local file, so a fresh clone had a dead reference in its own contributor
instructions. This check reads only TRACKED Markdown (git ls-files), so
local working material can never satisfy a reference in committed docs,
and fails when a backticked `docs/...md` / `spec/...md` path or a relative
Markdown link does not resolve to a tracked file.

Usage: python conformance/evidence/doc_links.py
Exit 0 when every reference resolves; exit 1 lists the dead ones.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# `docs/x.md` / `spec/y.md` in backticks, and relative Markdown links to
# such a file. Absolute URLs are out of scope.
BACKTICK = re.compile(r"`((?:docs|spec)/[A-Za-z0-9._/-]+\.md)`")
LINK = re.compile(r"\]\(((?:docs|spec)/[A-Za-z0-9._/-]+\.md)\)")


def tracked_markdown() -> list[Path]:
    out = subprocess.run(
        ["git", "ls-files", "*.md"], cwd=ROOT, capture_output=True, text=True
    )
    if out.returncode != 0:
        raise SystemExit("git ls-files failed; run from a checkout")
    return [Path(p) for p in out.stdout.splitlines() if p]


def tracked_files() -> set[str]:
    out = subprocess.run(["git", "ls-files"], cwd=ROOT, capture_output=True, text=True)
    return set(out.stdout.splitlines())


def main() -> int:
    tracked = tracked_files()
    dead: list[tuple[str, str]] = []
    for rel in tracked_markdown():
        text = (ROOT / rel).read_text(encoding="utf-8", errors="replace")
        for ref in BACKTICK.findall(text) + LINK.findall(text):
            if ref not in tracked:
                dead.append((str(rel), ref))
    for rel, ref in dead:
        print(f"dead reference: {rel} -> {ref}")
    print(f"doc-links: {len(dead)} dead of {len(tracked_markdown())} tracked files")
    return 0 if not dead else 1


if __name__ == "__main__":
    sys.exit(main())

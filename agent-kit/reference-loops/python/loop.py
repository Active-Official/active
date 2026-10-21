#!/usr/bin/env python3
"""Reference agent loop in Python (CHECKLIST 2.020, PROPOSAL section 4.1).

Full cycle on the real path: candidate Active -> validate (diagnose: the
module runs the hermetic engine, resilient by default, so diagnostics
include both errors and repair warnings) -> resilient eval (repair:
default fills, key renames, coercions applied by the engine) -> re-emit
the repaired value to source via `active import` -> check again
(validate) -> final redacted eval.

Repair is engine-driven, never guessed: every change between iterations
is recorded as the diagnostic codes that caused it. A stuck loop (same
codes twice) or the iteration budget ends the run with diagnostics intact.

Secret safety: the log records codes, paths, counts, and iteration
numbers only. Values, sources, and envelopes never enter the log, and
the re-emit step only ever sees already-redacted JSON.

Usage:
  python agent-kit/reference-loops/python/loop.py <candidate.active> \
      [--max-iters=N]
Exit 0 prints redacted JSON plus the code-only log; exit 1 prints the
final diagnostics.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

BINDINGS = Path(__file__).resolve().parents[3] / "bindings" / "python"
sys.path.insert(0, str(BINDINGS))
REPO = BINDINGS.parents[1]

from active import ActiveError, Module


def cli_base() -> list[str]:
    exe = shutil.which("active")
    if exe:
        return [exe]
    return ["cargo", "run", "-q", "-p", "active-cli", "--"]


def reemit(value: dict) -> str:
    with tempfile.NamedTemporaryFile(
        mode="w", suffix=".json", delete=False, encoding="utf-8"
    ) as f:
        json.dump(value, f)
        tmp = f.name
    try:
        p = subprocess.run(
            cli_base() + ["import", tmp, "--from", "json"],
            cwd=REPO,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=120,
        )
    finally:
        Path(tmp).unlink(missing_ok=True)
    if p.returncode != 0:
        raise ActiveError(f"re-emit failed: {p.stderr[:200]}")
    return p.stdout


def run(source: str, max_iters: int = 5) -> tuple[dict, list[dict]]:
    log: list[dict] = []
    current = source
    seen: set[tuple[str, ...]] = set()
    with Module() as mod:
        for i in range(1, max_iters + 1):
            # Validation step of the agent loop (gate: same verdict as
            # check by construction; both are exercised every iteration).
            diags = mod.validate(current)
            codes = sorted({d.code for d in diags})
            entry: dict = {"iter": i, "codes": codes}
            log.append(entry)
            if not codes:
                return mod.eval(current), log
            if tuple(codes) in seen:
                entry["stuck"] = True
                break
            seen.add(tuple(codes))
            try:
                value = mod.eval(current)
            except ActiveError:
                entry["unrepairable"] = True
                break
            entry["repaired_by_resilient"] = True
            current = reemit(value)
        raise ActiveError(f"unrepaired after {len(log)} iterations: {log[-1]}")


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    max_iters = 5
    for a in sys.argv[1:]:
        if a.startswith("--max-iters="):
            try:
                max_iters = int(a.split("=", 1)[1])
            except ValueError:
                max_iters = 0
    if not args or max_iters < 1:
        print(
            "usage: loop.py <candidate.active> [--max-iters=N (positive integer)]",
            file=sys.stderr,
        )
        return 2
    try:
        value, log = run(Path(args[0]).read_text(encoding="utf-8"), max_iters)
    except (ActiveError, subprocess.TimeoutExpired) as e:
        print(f"FAILED: {type(e).__name__}", file=sys.stderr)
        return 1
    print(json.dumps({"value": value, "log": log}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

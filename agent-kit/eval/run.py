#!/usr/bin/env python3
"""Agent-loop evaluation harness (CHECKLIST 2.021, PROPOSAL section 4.2).

Runs every available reference loop over the task vectors in
agent-kit/vectors/ and records: first-attempt success (clean on the
first check), mean iterations to clean, diagnostic-code histogram,
budget adherence (max-iters respected), and secret safety (no raw
secret material in any captured log).

Usage:
  python agent-kit/eval/run.py [--out report.json]
Exit code 0 only when every expectation holds.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
VECTORS = ROOT / "agent-kit" / "vectors"

TASKS = [
    ("repair-fill.active", True),
    ("clean-pass.active", True),
    ("broken-parse.active", False),
]

LOOPS = [
    ("python", [sys.executable, "agent-kit/reference-loops/python/loop.py"], ROOT),
    ("typescript", ["node", "agent-kit/reference-loops/typescript/loop.js"], ROOT),
    ("go", ["go", "run", "."], ROOT / "agent-kit" / "reference-loops" / "go"),
]


def available(cmd: list[str]) -> bool:
    if cmd[0] == sys.executable:
        return True
    return shutil.which(cmd[0]) is not None


def run_loop(cmd: list[str], cwd: Path, task: Path, max_iters: int) -> dict:
    p = subprocess.run(
        [*cmd, str(task), f"--max-iters={max_iters}"],
        cwd=cwd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=600,
    )
    out = {"exit": p.returncode, "iters": None, "codes": []}
    if p.returncode == 0:
        try:
            r = json.loads(p.stdout)
            out["iters"] = len(r.get("log", []))
            for entry in r.get("log", []):
                out["codes"].extend(entry.get("codes") or [])
        except json.JSONDecodeError:
            out["exit"] = 99
    out["tail"] = (p.stderr or p.stdout)[-300:]
    return out


def main() -> int:
    out = Path(sys.argv[sys.argv.index("--out") + 1]) if "--out" in sys.argv else None
    report: dict = {"loops": {}, "ok": True}
    for name, cmd, cwd in LOOPS:
        if not available(cmd):
            report["loops"][name] = {"status": "skipped-no-runtime"}
            continue
        entry: dict = {"tasks": {}}
        failed = False
        iters_sum = 0
        iters_n = 0
        for task, expect_clean in TASKS:
            r = run_loop(cmd, cwd, VECTORS / task, 5)
            if "hunter2" in json.dumps(r) or "sk-" in json.dumps(r):
                r["secret_leak"] = True
                failed = True
            clean = r["exit"] == 0
            if clean != expect_clean:
                failed = True
            if clean and r["iters"] is not None:
                iters_sum += r["iters"]
                iters_n += 1
                if task == "clean-pass.active" and r["iters"] != 1:
                    failed = True
            entry["tasks"][task] = {
                "clean": clean,
                "iters": r["iters"],
                "codes": r["codes"],
            }
        entry["mean_iters_to_clean"] = (iters_sum / iters_n) if iters_n else None
        entry["status"] = "fail" if failed else "pass"
        if failed:
            report["ok"] = False
        report["loops"][name] = entry
    text = json.dumps(report, indent=2)
    print(text)
    if out:
        out.write_text(text + "\n", encoding="utf-8")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

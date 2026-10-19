"""Score repair benchmarks (G-009): run `active check` on each broken
fixture in conformance/repair/ and award a point per fixture whose
required manifest codes reproduce. Usage: python tools/score-repair.py
Exit 0 always (benchmarks inform, never gate); the printed score tracks
repair-loop quality over time.
"""

import os
import re
import subprocess
import sys

try:
    import tomllib
except ImportError:
    tomllib = None

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ACTIVE = os.path.join(ROOT, "target", "debug", "active.exe")
if not os.path.isfile(ACTIVE):
    ACTIVE = os.path.join(ROOT, "target", "release", "active.exe")
REPAIR = os.path.join(ROOT, "conformance", "repair")

CODE_RE = re.compile(r"\b([A-Z][0-9]{3})\b")


def load_manifest():
    if tomllib is None:
        raise SystemExit("tomllib required (Python 3.11+)")
    with open(os.path.join(REPAIR, "manifest.toml"), "rb") as fh:
        return tomllib.load(fh)


def main():
    manifest = load_manifest()
    score = 0
    total = 0
    for item in manifest.get("repairs", []):
        total += 1
        path = os.path.join(REPAIR, item["file"])
        p = subprocess.run([ACTIVE, "check", path], capture_output=True, timeout=120)
        blob = p.stderr.decode("utf-8", "replace") + p.stdout.decode("utf-8", "replace")
        codes = []
        for c in CODE_RE.findall(blob):
            if c not in codes:
                codes.append(c)
        want = item["codes"]
        hit = all(c in codes for c in want)
        score += 1 if hit else 0
        print(
            "%s required=%s got=%s %s"
            % (item["id"], want, codes, "POINT" if hit else "MISS")
        )
    print("repair-bite-score: %d/%d" % (score, total))
    return 0


if __name__ == "__main__":
    sys.exit(main())

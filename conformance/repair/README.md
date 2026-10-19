# Repair benchmarks (G-009)

Each `repair-*.active` file is broken on purpose with a recorded code.
Repair it until `active check` exits 0, using only the diagnostic (code
plus span) as guidance. `tools/score-repair.py` scores a run: one point
per fixture whose required codes reproduce on the broken input. Track
scores over time to measure repair-loop quality; benchmarks inform, they
never gate releases and never change to flatter a score.

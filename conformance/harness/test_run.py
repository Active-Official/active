#!/usr/bin/env python3
"""Unit tests for the harness enforcement rules (CHECKLIST 3.001).

These tests pin the cross-implementation parity gate INSIDE
conformance/harness/run.py: identical codes, spans, and redacted JSON
across embeddings, with a failing overall run whenever the gate is
missing, red, or leaking. Weakening classify() or verify_parity() must
turn these tests red — that is the point: the gate cannot be quietly
relaxed.

Run from the repo root: ``python conformance/harness/test_run.py``.
CI runs this file before the shared harness (see conformance.yml).
"""

from __future__ import annotations

import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from run import PATHS, TIER1, classify, finalize_ok, verify_parity


def passing_report() -> dict:
    # Built from the live PATHS table so the fixture cannot drift from
    # the harness it tests: Tier-1 paths pass, everything else skips
    # openly. (The differential entry below stands in for a real
    # DIFFERENTIAL-OK run; diff_vectors.py proves the real thing.)
    paths = {}
    for name, _, _, _ in PATHS:
        if name in TIER1:
            paths[name] = {"status": "pass", "secret_clean": True}
        else:
            paths[name] = {"status": "skipped", "secret_clean": True}
    return {"paths": paths, "secret_leak": False, "ok": True}


class ClassifyTest(unittest.TestCase):
    def test_tally_match_is_pass(self) -> None:
        status, tally, clean = classify(0, "js-ok\n", r"^js-ok$")
        self.assertEqual((status, tally, clean), ("pass", "js-ok", True))

    def test_missing_tally_is_fail(self) -> None:
        status, _, _ = classify(0, "something else\n", r"^js-ok$")
        self.assertEqual(status, "fail")

    def test_skip_clean_exit_skips(self) -> None:
        status, _, clean = classify(0, "SKIP: zig not installed\n", r"^SMOKE-OK$")
        self.assertEqual(status, "skipped")
        self.assertTrue(clean)

    def test_skip_nonzero_exit_fails(self) -> None:
        # A runner that prints SKIP then fails must fail the run, never
        # pass silently. This is the P3-2 hardening; deleting the
        # code==0 requirement turns this test red.
        status, _, _ = classify(1, "SKIP: ext-ffi not available\n", r"^SMOKE-OK$")
        self.assertEqual(status, "fail")

    def test_missing_binary_skips(self) -> None:
        status, _, _ = classify(127, "MISSING BINARY: cargo\n", r"^test result: ok\.")
        self.assertEqual(status, "skipped")

    def test_raw_secret_never_counts_clean(self) -> None:
        # The leak gate reads RAW output: checking after scrub() would
        # pass every run by construction (old P3-1 dead code).
        status, _, clean = classify(0, "token hunter2 ok\n", r"^token .* ok$")
        self.assertFalse(clean)
        self.assertEqual(status, "fail")


class ParityTest(unittest.TestCase):
    def test_all_passing_report_holds(self) -> None:
        self.assertEqual(verify_parity(passing_report()), [])

    def test_missing_differential_gate_fails(self) -> None:
        # Deleting the differential path from PATHS must fail the run,
        # not silently drop the parity proof.
        report = passing_report()
        del report["paths"]["differential"]
        violations = verify_parity(report)
        self.assertTrue(
            any("differential" in v for v in violations),
            f"expected differential violation, got {violations}",
        )

    def test_red_differential_gate_fails(self) -> None:
        report = passing_report()
        report["paths"]["differential"] = {
            "status": "fail",
            "secret_clean": True,
        }
        violations = verify_parity(report)
        self.assertTrue(
            any("differential" in v for v in violations),
            f"expected differential violation, got {violations}",
        )

    def test_leaking_differential_gate_fails(self) -> None:
        report = passing_report()
        report["paths"]["differential"] = {
            "status": "pass",
            "secret_clean": False,
        }
        self.assertTrue(verify_parity(report))

    def test_failing_path_fails_parity(self) -> None:
        report = passing_report()
        report["paths"]["wasm"] = {"status": "fail", "secret_clean": True}
        violations = verify_parity(report)
        self.assertTrue(any("wasm" in v for v in violations), violations)

    def test_secret_leak_fails_parity(self) -> None:
        report = passing_report()
        report["secret_leak"] = True
        violations = verify_parity(report)
        self.assertTrue(any("leak" in v for v in violations), violations)

    def test_tier1_set_matches_wired_paths(self) -> None:
        # Every TIER1 name must exist in PATHS: renaming or deleting a
        # Tier-1 entry must turn this red, never silently shrink the gate.
        wired = {name for name, _, _, _ in PATHS}
        self.assertTrue(
            TIER1 <= wired, f"Tier-1 paths missing from PATHS: {TIER1 - wired}"
        )

    def test_skipped_tier1_path_fails_parity(self) -> None:
        # Tier-2 paths may skip openly, but a Tier-1 embedding that
        # skips (missing toolchain, unbuilt lib) is a failed run: there
        # is no parity proof without it.
        report = passing_report()
        report["paths"]["python"] = {"status": "skipped", "secret_clean": True}
        violations = verify_parity(report)
        self.assertTrue(any("python" in v for v in violations), violations)

    def test_missing_tier1_path_fails_parity(self) -> None:
        report = passing_report()
        del report["paths"]["c_abi"]
        violations = verify_parity(report)
        self.assertTrue(any("c_abi" in v for v in violations), violations)

    def test_timeout_always_fails(self) -> None:
        # A timeout fails even when the partial output happens to carry
        # a SKIP line or a matching tally.
        for out in ("SKIP: zig not installed\n", "SMOKE-OK\n"):
            status, _, _ = classify(0, out, r"^SMOKE-OK$", timed_out=True)
            self.assertEqual(status, "fail")

    def test_placeholders_never_count(self) -> None:
        import run as harness

        report = passing_report()
        old, harness.PLACEHOLDERS = harness.PLACEHOLDERS, ["future-lang"]
        try:
            report["paths"]["future-lang"] = {"status": "not-implemented"}
            # Placeholders must not create violations on their own.
            self.assertEqual(verify_parity(report), [])
        finally:
            harness.PLACEHOLDERS = old

    def test_skipped_differential_fails_parity(self) -> None:
        # A differential path that cannot run is not a passing proof:
        # missing toolchains must fail loudly, never silently pass. This
        # pins the strictness intentionally (local runs need built libs).
        report = passing_report()
        report["paths"]["differential"] = {
            "status": "skipped",
            "secret_clean": True,
        }
        violations = verify_parity(report)
        self.assertTrue(any("differential" in v for v in violations), violations)

    def test_missing_secret_clean_flag_fails(self) -> None:
        # Entries must state secret_clean explicitly; a missing flag
        # defaults closed (fail), never open.
        report = passing_report()
        del report["paths"]["wasm"]["secret_clean"]
        violations = verify_parity(report)
        self.assertTrue(any("wasm" in v for v in violations), violations)


class FinalizeTest(unittest.TestCase):
    def test_clean_run_is_ok(self) -> None:
        self.assertTrue(finalize_ok(False, []))

    def test_path_failure_is_not_ok(self) -> None:
        self.assertFalse(finalize_ok(True, []))

    def test_parity_violation_is_not_ok(self) -> None:
        # Bypassing verify_parity in main() must turn this red: the
        # overall verdict has to consult the parity gate.
        self.assertFalse(finalize_ok(False, ["differential missing"]))


class RegistryTest(unittest.TestCase):
    def test_wired_paths_complete(self) -> None:
        # Every coverage path must stay registered: deleting a PATHS
        # entry (Tier-1 or Tier-2) silently shrinks the gate, so adding
        # or removing a binding requires updating this list deliberately.
        expected = {
            "reference",
            "c_abi",
            "wasm",
            "go",
            "python",
            "js-native",
            "zig",
            "csharp",
            "cpp",
            "java",
            "lua",
            "ruby",
            "php",
            "fortran",
            "go-native",
            "haskell",
            "julia",
            "nim",
            "perl",
            "r",
            "ocaml",
            "crystal",
            "elixir",
            "dart",
            "vlang",
            "kotlin",
            "swift",
            "differential",
        }
        wired = {name for name, _, _, _ in PATHS}
        self.assertEqual(wired, expected)
        # Tier-1 set: no extras, no omissions vs the wired table.
        self.assertTrue(
            TIER1 <= wired, f"Tier-1 paths missing from PATHS: {TIER1 - wired}"
        )


class ExpectationFilesTest(unittest.TestCase):
    def test_error_files_stored_sorted_set(self) -> None:
        # NORMALIZATION.md section 1 claims committed expectations are
        # stored sorted by convention; enforce it so the live comparison
        # and the stored files cannot drift apart silently.
        root = Path(__file__).resolve().parents[2]
        files = sorted((root / "conformance" / "errors").glob("*.json"))
        self.assertTrue(files, "no committed expectation files found")
        for f in files:
            codes = json.loads(f.read_text(encoding="utf-8"))["codes"]
            self.assertEqual(codes, sorted(set(codes)), f.name)

    def test_non_refused_cases_have_expected_values(self) -> None:
        # Static half of the missing-value fail-closed rule: every
        # clean/diagnosed non-pinned case must ship its committed value
        # file. Manifest kind "refused" is checked live instead of here:
        # strict violations (T003/T005/E023) surface as
        # Ok-plus-diagnostics with a value (see 0007/0010), and the live
        # gate fails any value-yielding case without its file
        # (check_values, pinned by test_diff_vectors.py).
        import tomllib

        root = Path(__file__).resolve().parents[2]
        manifest = tomllib.loads(
            (root / "conformance" / "manifest.toml").read_text(encoding="utf-8")
        )
        pins = {p["id"] for p in manifest.get("pins", [])}
        for case in manifest.get("cases", []):
            cid = case["id"]
            if cid in pins or case.get("kind") == "refused":
                continue
            self.assertTrue(
                (root / "conformance" / "expected" / f"{cid}.json").is_file(),
                f"non-refused case {cid} lacks expected value file",
            )


if __name__ == "__main__":
    unittest.main(verbosity=2)

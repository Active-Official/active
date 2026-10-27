#!/usr/bin/env python3
"""Unit tests for the differential gate helpers (CHECKLIST Step B.003).

Pins the pure comparison rules inside conformance/harness/diff_vectors.py:
sorted-set expectation view, secret scrubbing, and manifest pin loading.
The full cross-embedding proof itself runs live (DIFFERENTIAL-OK over the
hermetic manifest); these tests stop the helpers from drifting under it.

Run from the repo root: ``python conformance/harness/test_diff_vectors.py``.
CI runs this file before the shared harness (see conformance.yml).
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from diff_vectors import (
    check_values,
    fail,
    load_pins,
    norm_codes,
    parse_null_problems,
    scrub,
)


class NormCodesTest(unittest.TestCase):
    def test_order_and_duplicates_collapse(self) -> None:
        self.assertEqual(norm_codes(["E004", "E002", "E004"]), ["E002", "E004"])

    def test_empty_stays_empty(self) -> None:
        self.assertEqual(norm_codes([]), [])

    def test_single_sorted_trivially(self) -> None:
        self.assertEqual(norm_codes(["T001"]), ["T001"])


class ScrubTest(unittest.TestCase):
    def test_raw_marker_redacted(self) -> None:
        self.assertNotIn("hunter2", scrub("token hunter2 ok"))
        self.assertIn("***SECRET***", scrub("token hunter2 ok"))

    def test_clean_text_untouched(self) -> None:
        self.assertEqual(scrub("DIFF 0001 ok"), "DIFF 0001 ok")

    def test_fail_output_is_scrubbed(self) -> None:
        import io
        from contextlib import redirect_stdout

        buf = io.StringIO()
        with redirect_stdout(buf):
            fail("leak hunter2 here")
        self.assertNotIn("hunter2", buf.getvalue())
        self.assertIn("DIFFERENTIAL-FAIL", buf.getvalue())


class PinsTest(unittest.TestCase):
    def test_load_pins_maps_id_to_reason(self) -> None:
        pins = load_pins({"pins": [{"id": "0008", "reason": "loader"}]})
        self.assertEqual(pins, {"0008": "loader"})

    def test_load_pins_empty_without_section(self) -> None:
        self.assertEqual(load_pins({}), {})


class CheckValuesTest(unittest.TestCase):
    def test_refused_with_no_values_is_clean(self) -> None:
        self.assertEqual(check_values(True, (None, None, None), True, None), [])

    def test_refused_with_a_value_fails(self) -> None:
        problems = check_values(True, ({"a": 1}, {"a": 1}, {"a": 1}), True, None)
        self.assertTrue(any("refused but a value" in p for p in problems))

    def test_valued_case_without_expectation_file_fails(self) -> None:
        # The 0007 lesson: agreeing-but-unpinned values must fail closed.
        v = ({"port": 0}, {"port": 0}, {"port": 0})
        problems = check_values(False, v, True, None)
        self.assertTrue(any("without committed expected" in p for p in problems))

    def test_valued_case_matching_expectation_passes(self) -> None:
        v = ({"port": 0}, {"port": 0}, {"port": 0})
        self.assertEqual(check_values(False, v, False, {"port": 0}), [])

    def test_valued_case_diverging_embeddings_fail(self) -> None:
        v = ({"port": 0}, {"port": 1}, {"port": 0})
        problems = check_values(False, v, False, {"port": 0})
        self.assertTrue(any("diverge" in p for p in problems))

    def test_valued_case_mismatching_expectation_fails(self) -> None:
        v = ({"port": 0}, {"port": 0}, {"port": 0})
        problems = check_values(False, v, False, {"port": 1})
        self.assertTrue(any("committed expectation" in p for p in problems))



class ParseNullContractTest(unittest.TestCase):
    """The C ABI parse-refusal branch: numeric detail only, no value."""

    def call(self, **over):
        base = dict(
            kind="refused",
            py_codes=["E002"],
            js_codes=["E002"],
            py_spans=[(0, 1, 0, 2)],
            js_spans=[(0, 1, 0, 2)],
            py_sev=["error"],
            js_sev=["error"],
            py_refused=True,
            js_refused=True,
            py_value=None,
            js_value=None,
            want_codes=["E002"],
        )
        base.update(over)
        return parse_null_problems(**base)

    def test_agreeing_refusal_is_clean(self) -> None:
        self.assertEqual(self.call(), [])

    def test_divergent_codes_fail(self) -> None:
        self.assertTrue(any("codes diverge" in p for p in self.call(js_codes=["E003"])))

    def test_kind_clean_with_codes_fails(self) -> None:
        problems = self.call(kind="clean")
        self.assertTrue(any("kind=clean but codes surfaced" in p for p in problems))
        self.assertTrue(any("kind=clean but an embedding refused" in p for p in problems))

    def test_value_after_parse_refusal_fails(self) -> None:
        self.assertTrue(
            any("value came back" in p for p in self.call(py_value={"a": 1}))
        )

    def test_secret_marker_fails(self) -> None:
        self.assertTrue(
            any("raw secret marker" in p for p in self.call(py_value="hunter2-SECRET"))
        )

    def test_refused_without_codes_fails(self) -> None:
        self.assertTrue(
            any("no codes surfaced" in p for p in self.call(py_codes=[], js_codes=[]))
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)

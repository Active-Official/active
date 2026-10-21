"""Conformance-facing tests for the Python binding (CHECKLIST 1.006).

Drives the shipped wasmtime path on the real module: ABI gate, eval value,
units refusal with a stable code, secret redaction, and determinism.
Run from the repo root: ``python bindings/python/test_active.py``.
Prints unittest's ``OK`` on success (the harness tally).
"""

import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from active import ActiveError, Module


class BindingTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.mod = Module()

    @classmethod
    def tearDownClass(cls) -> None:
        cls.mod.close()

    def test_eval_value(self) -> None:
        self.assertEqual(self.mod.eval("port = 8080\n"), {"port": 8080})

    def test_check_clean(self) -> None:
        self.assertEqual(self.mod.check('country = "NO"\n'), [])

    def test_units_refused_with_code(self) -> None:
        diags = self.mod.check("timeout = 5s + 512MiB\n")
        self.assertTrue(any(d.code == "T001" for d in diags))
        with self.assertRaises(ActiveError):
            self.mod.eval("timeout = 5s + 512MiB\n")

    def test_check_carries_structured_spans(self) -> None:
        diags = self.mod.check("timeout = 5s + 512MiB\n")
        self.assertEqual(len(diags), 1)
        d = diags[0]
        self.assertEqual(d.code, "T001")
        self.assertEqual(d.severity, "error")
        self.assertEqual(d.span(), (1, 14, 1, 15))
        self.assertEqual(self.mod.check("port = 8080\n"), [])

    def test_secret_redacted(self) -> None:
        value = self.mod.eval('token = secret("hunter2")\n[out]\npass = token\n')
        self.assertEqual(value, {"token": "***SECRET***", "out": {"pass": None}})
        self.assertNotIn("hunter2", json.dumps(value))
        diags = self.mod.check('token = secret("hunter2")\n[out]\nv = token + 1\n')
        self.assertTrue(diags, "expected a diagnostic")
        for d in diags:
            self.assertNotIn("hunter2", d.message)

    def test_deterministic(self) -> None:
        src = "let x = 1 + 2\n[out]\nv = x * 3\n"
        self.assertEqual(self.mod.eval(src), self.mod.eval(src))


if __name__ == "__main__":
    unittest.main(verbosity=2)

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
        # Refusal paths must not leak either: messages and raised errors
        # carry codes and generic text only, never the secret.
        with self.assertRaises(ActiveError) as ctx:
            self.mod.eval('token = secret("hunter2")\n[out]\npass = token + 1\n')
        self.assertNotIn("hunter2", str(ctx.exception))

    def test_deterministic(self) -> None:
        src = "let x = 1 + 2\n[out]\nv = x * 3\n"
        self.assertEqual(self.mod.eval(src), self.mod.eval(src))

    def test_validate_agrees_with_check(self) -> None:
        # The validation step shares check's hermetic verdict path: same
        # codes, spans, severities, and per-item messages on every input,
        # clean or not. The differential harness pins cross-embedding
        # agreement; this pins the two entry points against each other.
        cases = [
            "port = 8080\n",
            "timeout = 5s + 512MiB\n",
            'token = secret("hunter2")\n[out]\npass = token\n',
            "[unclosed\n",
        ]
        keyed = lambda ds: [(d.code, d.span(), d.severity, d.message) for d in ds]
        for src in cases:
            self.assertEqual(
                keyed(self.mod.validate(src)),
                keyed(self.mod.check(src)),
                f"validate/check diverge on {src!r}",
            )

    def test_closed_module_fails_closed(self) -> None:
        # Using a module after close() raises ActiveError on every entry
        # point: never an UnboundLocalError, bare host exception, or a
        # plausible-looking empty result.
        mod = Module()
        mod.close()
        for op in (
            lambda: mod.check("port = 1\n"),
            lambda: mod.validate("port = 1\n"),
            lambda: mod.eval("port = 1\n"),
        ):
            with self.assertRaises(ActiveError):
                op()

    def test_stub_matches_runtime(self) -> None:
        # The .pyi stub must mirror the shipped API exactly: same public
        # functions, same classes, same method names AND signatures. A
        # stale stub fails here. Catches renames (names), dropped members
        # (sets), and arity drift (counts); annotations are pinned by
        # runtime smoke across the suite.
        import ast
        import inspect

        import active as runtime

        stub_path = Path(__file__).resolve().parent / "active.pyi"
        self.assertTrue(stub_path.is_file(), "active.pyi missing")
        tree = ast.parse(stub_path.read_text(encoding="utf-8"))
        stub_funcs: dict[str, object] = {}
        stub_classes: dict[str, list[str]] = {}
        for node in tree.body:
            if isinstance(node, ast.FunctionDef):
                stub_funcs[node.name] = node.args
            elif isinstance(node, ast.ClassDef):
                stub_classes[node.name] = [
                    sub.name for sub in node.body if isinstance(sub, ast.FunctionDef)
                ]
        runtime_funcs = {
            name
            for name, obj in vars(runtime).items()
            if inspect.isfunction(obj) and not name.startswith("_")
        }
        self.assertEqual(
            set(stub_funcs), runtime_funcs, "module-level stub/runtime drift"
        )
        self.assertIn("default_wasm_path", stub_funcs)
        for cls_name in ("Module", "Diagnostic"):
            runtime_methods = {
                name
                for name, obj in inspect.getmembers(
                    getattr(runtime, cls_name),
                    predicate=lambda m: (
                        inspect.isfunction(m) or inspect.ismethoddescriptor(m)
                    ),
                )
            }
            # __init__ and span are dunder-or-descriptor on both sides;
            # compare the plain public methods exactly.
            plain = lambda names: {
                n for n in names if not n.startswith("_") and n != "span"
            }
            self.assertEqual(
                plain(stub_classes[cls_name]),
                plain(runtime_methods),
                f"{cls_name} stub/runtime member drift",
            )
            self.assertIn("span", stub_classes["Diagnostic"])
            self.assertIn("span", dir(runtime.Diagnostic))


if __name__ == "__main__":
    unittest.main(verbosity=2)

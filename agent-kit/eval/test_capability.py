#!/usr/bin/env python3
"""Capability descriptor authority test (CHECKLIST E.004, PROPOSAL §5.4).

`capability.schema.json` is the machine-readable tool surface:
versioned, with every Tier-1 agent entry point covered. Consumers
reject unknown major versions; this test pins major == 1 and the
full tool list so neither can drift silently.

Run from the repo root: ``python agent-kit/eval/test_capability.py``.
CI runs this before the eval harness (see conformance.yml).
"""

from __future__ import annotations

import json
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCHEMA = ROOT / "agent-kit" / "capability.schema.json"


class CapabilityTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.doc = json.loads(SCHEMA.read_text(encoding="utf-8"))

    def test_version_major_is_one(self) -> None:
        version = self.doc.get("version")
        self.assertIsInstance(version, str, "version must be a string")
        major = version.split(".")[0]
        self.assertEqual(major, "1", f"unknown major version: {version}")

    def test_tool_surface_complete(self) -> None:
        names = [t.get("name") for t in self.doc.get("tools", [])]
        for required in (
            "check",
            "eval",
            "format",
            "convert",
            "graph",
            "generate",
            "schema-introspect",
        ):
            self.assertIn(required, names, f"tool surface missing {required}")

    def test_every_tool_described(self) -> None:
        for tool in self.doc.get("tools", []):
            self.assertTrue(tool.get("name"), "nameless tool")
            self.assertTrue(
                tool.get("description"), f"{tool.get('name')}: no description"
            )


if __name__ == "__main__":
    unittest.main(verbosity=2)

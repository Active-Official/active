#!/usr/bin/env python3
"""Vector tests for the hash short-circuit (profile §7)."""

from __future__ import annotations

import json
import unittest
from pathlib import Path

from short_circuit import missing_identities, normalize_identity, short_circuits

ROOT = Path(__file__).resolve().parent
VECTOR = ROOT / "vectors" / "short-circuit.json"

HELD = "blake3:1d9e18dd83b0c5b5d5e5f2f42195a2c32e0ce2b07e7edee5f0a421f70ba7324f"
MISSING = "blake3:abababababababababababababababababababababababababababababababab"


class ShortCircuitTest(unittest.TestCase):
    def test_vector_matches_the_shipped_example_identity(self) -> None:
        vector = json.loads(VECTOR.read_text(encoding="utf-8"))
        known = set(vector["known"])
        # The held identity is the real blake3 digest of the world-model
        # example's payload, so the vector is tied to shipped bytes.
        self.assertIn(HELD, known)
        self.assertEqual(missing_identities(vector["payload"], known), [MISSING])

    def test_held_identity_short_circuits_and_taint_does_not_change_it(self) -> None:
        vector = json.loads(VECTOR.read_text(encoding="utf-8"))
        known = set(vector["known"])
        self.assertTrue(short_circuits(vector["payload"]["model"], known))
        self.assertFalse(short_circuits(vector["payload"]["extra"], known))
        # A tainted blob still short-circuits when the receiver holds
        # the bytes: identity is content, not classification.
        tainted = {"$active.blob": {"hash": MISSING, "tainted": True}}
        self.assertTrue(short_circuits(tainted, {MISSING}))

    def test_malformed_identity_never_matches(self) -> None:
        for value in (
            None,
            {},
            {"hash": "sha256:aa"},
            {"hash": "blake3:"},
            {"hash": "blake3:" + "g" * 64},
            {"hash": "blake3:" + "a" * 63},
        ):
            self.assertIsNone(normalize_identity(value), value)

    def test_nested_payloads_are_walked_once(self) -> None:
        record = {"$active.blob": {"hash": MISSING, "size": 1}}
        payload = {"a": [record, {"b": record}], "c": {"d": record}}
        self.assertEqual(missing_identities(payload, set()), [MISSING])


if __name__ == "__main__":
    unittest.main(verbosity=2)

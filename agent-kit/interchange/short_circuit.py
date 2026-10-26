#!/usr/bin/env python3
"""Hash short-circuit helpers for the interchange profile (§7).

A receiver that already holds a content-addressed value can answer
"I already possess hash H" instead of asking for a transfer. Identity
is the blob's `blake3:` digest, so taint, size accounting, and media
type never change which bytes a hash names.

Run the vector test: ``python agent-kit/interchange/test_short_circuit.py``.
"""

from __future__ import annotations

import json
from pathlib import Path

DIGEST_PREFIX = "blake3:"
DIGEST_HEX = 64


def normalize_identity(value: object) -> str | None:
    """The ``blake3:<hex>`` identity of a content-addressed record.

    Accepts either an exported record (``{"$active.blob": {...}}``) or
    the inner record itself. Returns ``None`` for anything that is not
    a well-formed blob identity, so a malformed record is never
    mistaken for a short-circuit hit.
    """
    if not isinstance(value, dict):
        return None
    record = value.get("$active.blob")
    if not isinstance(record, dict):
        record = value
    digest = record.get("hash")
    if not isinstance(digest, str) or not digest.startswith(DIGEST_PREFIX):
        return None
    hex_part = digest[len(DIGEST_PREFIX) :]
    if len(hex_part) != DIGEST_HEX or any(
        c not in "0123456789abcdef" for c in hex_part
    ):
        return None
    return digest


def short_circuits(value: object, known: set[str]) -> bool:
    """Whether ``value`` names bytes the receiver already holds."""
    identity = normalize_identity(value)
    return identity is not None and identity in known


def missing_identities(payload: object, known: set[str]) -> list[str]:
    """Sorted identities reachable in ``payload`` that ``known`` lacks.

    Arrays and tables are walked in full; a record is a leaf (its
    payload bytes are never inspected, which is what keeps a tainted
    blob from forcing a transfer).
    """
    missing: set[str] = set()

    def walk(value: object) -> None:
        identity = normalize_identity(value)
        if identity is not None:
            if identity not in known:
                missing.add(identity)
            return
        if isinstance(value, dict):
            for child in value.values():
                walk(child)
        elif isinstance(value, list):
            for child in value:
                walk(child)

    walk(payload)
    return sorted(missing)


def main() -> int:
    root = Path(__file__).resolve().parent
    vector = json.loads(
        (root / "vectors" / "short-circuit.json").read_text(encoding="utf-8")
    )
    known = set(vector["known"])
    missing = missing_identities(vector["payload"], known)
    if missing != vector["missing"]:
        print(f"FAIL: missing identities {missing} != {vector['missing']}")
        return 1
    held = vector["payload"]["model"]
    if not short_circuits(held, known):
        print("FAIL: a held identity did not short-circuit")
        return 1
    print("SHORT-CIRCUIT-OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

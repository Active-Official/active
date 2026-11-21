#!/usr/bin/env python3
"""Batch-validate JSON values against generated JSON Schemas (L-P25).

The Rust differential test (`active-core/tests/generated_contracts.rs`)
drives the shipped generator and evaluator, then hands the generated
schemas and wire values to this script so the verdicts come from the
same pinned validator the rest of the tooling uses (CI installs
`jsonschema==4.26.0`; see `.github/workflows/conformance.yml`).

Usage:
    python conformance/evidence/jsonschema_batch.py <input.json> <output.json>

Input:  [{"schema": <schema>, "values": [<value>, ...]}, ...]
Output: [[bool, ...], ...] in the same order (True = accepted).
"""

from __future__ import annotations

import json
import sys

import jsonschema


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: jsonschema_batch.py <input.json> <output.json>", file=sys.stderr)
        return 2
    with open(sys.argv[1], encoding="utf-8") as handle:
        batches = json.load(handle)
    verdicts: list[list[bool]] = []
    for batch in batches:
        schema = batch["schema"]
        # The generated contract must itself be a valid draft-07 schema.
        jsonschema.Draft7Validator.check_schema(schema)
        # L-P25/F4: no format checker. The generated schema carries Active's
        # own regex as `pattern` for every named format, so a second,
        # narrower checker (Python's `uri`/`uuid` semantics differ) would
        # reject values Active accepts and blur which constraint failed.
        validator = jsonschema.Draft7Validator(schema)
        verdicts.append([validator.is_valid(value) for value in batch["values"]])
    with open(sys.argv[2], "w", encoding="utf-8") as handle:
        json.dump(verdicts, handle)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

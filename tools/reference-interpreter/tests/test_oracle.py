"""Unit tests for the L-P37 reference interpreter itself.

These pin fixed vectors and the review-driven invariants the differential
would only catch when a Monte Carlo family happens to generate them:
blob v1/v2 identities, env-derived taint, dotted requires, tagged
non-finite export, truncating division, the all-node cycle check, the
depth budget, and strict `major.minor.patch` metadata.

Run from the repository root:
    python -m unittest discover -s tools/reference-interpreter/tests
"""

from __future__ import annotations

import json
import math
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from active_ref.api import evaluate_file, evaluate_source
from active_ref.eval import Budget, Engine, _semver, _version_at_least
from active_ref.export import content_hash, float_text, to_json_value
from active_ref.model import Value

UNTAINTED_BLOB = (
    "blake3:ef8a8525ffd1d32141916f461ccb49c92ee84d57279145268364fb7c1c179eea"
)
TAINTED_BLOB = "blake3:ab0710f2bf86f2984ecca1ca1e14e6d941bd3c614e1574ff5ac8c7ed7451fb73"


def records(outcome) -> list[tuple[str, str | None]]:
    return [(diag.code, diag.path) for diag in outcome.diagnostics]


class BlobIdentityTests(unittest.TestCase):
    def test_salt_matches_shipped_for_schema_and_profile(self) -> None:
        body = (
            's = secret("payload-secret")\nb = blob("text/plain", "payload-secret")\n'
        )
        schema = evaluate_source(
            'schema {\n  x = String\n}\nx = "a"\n' + body, "salt-schema"
        )
        self.assertEqual(
            to_json_value(schema.value)["b"]["$active.blob"]["hash"],
            "blake3:82828c8f93e37d9c1eae9233e8b53210d6c27121e22d864919711a61f86dcb88",
        )
        profile = evaluate_source('profile "p" {\n  y = 2\n}\n' + body, "salt-profile")
        self.assertEqual(
            to_json_value(profile.value)["b"]["$active.blob"]["hash"],
            "blake3:200b16bb6b6ca09e3ca6d4034354d83ebd3bc2f876a67c9dff0089ef40a4926a",
        )

    def test_untainted_blob_uses_the_v1_vector(self) -> None:
        outcome = evaluate_source('b = blob("text/plain", "hello")\n', "v1")
        self.assertEqual(outcome.diagnostics, [])
        blob = to_json_value(outcome.value)["b"]["$active.blob"]
        self.assertEqual(blob["hash"], UNTAINTED_BLOB)
        self.assertFalse(blob["tainted"])
        self.assertEqual(blob["media_type"], "text/plain")

    def test_tainted_blob_uses_the_salted_v2_vector(self) -> None:
        source = (
            'x = secret("payload-secret")\nb = blob("text/plain", "payload-secret")\n'
        )
        outcome = evaluate_source(source, "v2")
        self.assertEqual(outcome.diagnostics, [])
        blob = to_json_value(outcome.value)["b"]["$active.blob"]
        self.assertEqual(blob["hash"], TAINTED_BLOB)
        self.assertTrue(blob["tainted"])
        self.assertEqual(blob["media_type"], "***SECRET***")


class TaintTests(unittest.TestCase):
    def test_env_derived_secret_taints_a_blob_and_never_echoes(self) -> None:
        source = 's = secret(env.MY_SECRET)\nb = blob("text/plain", s)\n'
        outcome = evaluate_source(
            source, "env-secret", Engine(env={"MY_SECRET": "payload-secret"})
        )
        self.assertEqual(outcome.diagnostics, [])
        rendered = json.dumps(to_json_value(outcome.value))
        self.assertNotIn("payload-secret", rendered)
        blob = json.loads(rendered)["b"]["$active.blob"]
        self.assertTrue(blob["tainted"])
        self.assertEqual(blob["media_type"], "***SECRET***")
        self.assertEqual(blob["size"], 0)

    def test_derived_scalars_stay_material_in_every_form(self) -> None:
        source = (
            's = secret("payload-secret")\n'
            'c = s == "x"\n'
            't = "v=${c}"\n'
            "arr = [c]\n"
            "d = c == c\n"
            "e = c && true\n"
        )
        outcome = evaluate_source(source, "derived-material")
        self.assertEqual(outcome.diagnostics, [])
        value = to_json_value(outcome.value)
        self.assertEqual(
            value,
            {
                "s": "***SECRET***",
                "c": "***SECRET***",
                "t": "***SECRET***",
                "arr": ["***SECRET***"],
                "d": "***SECRET***",
                "e": "***SECRET***",
            },
        )

    def test_copied_substrings_and_keys_are_redacted(self) -> None:
        source = 's = secret("payload-secret")\nt = "x-payload-secret-y"\nfoo = 1\n'
        outcome = evaluate_source(source, "substring-copy")
        value = to_json_value(outcome.value, seeds=outcome.seeds)
        self.assertEqual(value["t"], "x-***SECRET***-y")
        keyed = evaluate_source('s = secret("foo")\nfoo = 1\n', "key")
        self.assertIn("***SECRET***", to_json_value(keyed.value, seeds=keyed.seeds))

    def test_a_plain_profile_override_at_a_secret_path_stays_material(self) -> None:
        source = (
            'a = secret("hunter2")\n'
            'profile "dev" {\n'
            "  a = 1\n"
            "}\n"
            'active_profile = "dev"\n'
        )
        outcome = evaluate_source(source, "profile-secret")
        value = to_json_value(outcome.value, seeds=outcome.seeds)
        self.assertEqual(value["a"], "***SECRET***")

    def test_a_blob_over_a_derived_scalar_is_tainted(self) -> None:
        source = (
            's = secret("payload-secret")\n'
            'c = s == "x"\n'
            't = "v=${c}"\n'
            'b = blob("text/plain", t)\n'
        )
        outcome = evaluate_source(source, "derived-blob")
        self.assertEqual(outcome.diagnostics, [])
        rendered = json.dumps(to_json_value(outcome.value))
        self.assertNotIn("payload-secret", rendered)
        blob = json.loads(rendered)["b"]["$active.blob"]
        self.assertTrue(blob["tainted"])
        self.assertEqual(blob["media_type"], "***SECRET***")
        self.assertEqual(blob["size"], 7)


class DiagnosticRecordTests(unittest.TestCase):
    def test_span_starts_at_the_unknown_unit_run(self) -> None:
        outcome = evaluate_source("bad0 = 2e0\n", "unit-span")
        self.assertEqual(
            [
                (d.code, d.span.line, d.span.column, d.span.end_column)
                for d in outcome.diagnostics
            ],
            [("E002", 1, 9, 11)],
        )

    def test_duplicate_diagnostics_are_kept(self) -> None:
        source = (
            "schema {\n"
            "  config = Table { flag = Bool }\n"
            '  user = String(requires = "config.flag")\n'
            "}\n"
            "config = { flag = true }\n"
        )
        outcome = evaluate_source(source, "duplicate-t004")
        # The engine reports a triggered missing key once; the CLI adds a
        # summary line, which is not part of the diagnostic record.
        self.assertEqual(
            [(d.code, d.path) for d in outcome.diagnostics],
            [("T004", "user")],
        )


class DynamicBudgetTests(unittest.TestCase):
    def test_alias_amplification_of_an_env_value_trips_the_total(self) -> None:
        source = "".join(
            f"a{i} = " + ("env.BIG" if i == 0 else f"a{i - 1}") + "\n"
            for i in range(20)
        )
        outcome = evaluate_source(
            source,
            "amplify",
            Engine(
                env={"BIG": "x" * 10_000},
                budget=Budget(max_total_value_size=50_000),
            ),
        )
        self.assertEqual(records(outcome), [("X006", None)])
        self.assertEqual(
            (
                outcome.diagnostics[0].span.line,
                outcome.diagnostics[0].span.column,
            ),
            (5, 6),
        )

    def test_patch_composed_depth_is_checked(self) -> None:
        source = 'base = { a = { b = 1 } }\n@patch("base.a.c", 2)\n'
        outcome = evaluate_source(
            source, "patch-depth", Engine(budget=Budget(max_depth=2))
        )
        self.assertEqual(records(outcome), [("X003", None)])

    def test_expression_nodes_do_not_deepen_the_value(self) -> None:
        source = "a = " + " + ".join(["1"] * 40) + "\n"
        outcome = evaluate_source(
            source, "binary-depth", Engine(budget=Budget(max_depth=1))
        )
        self.assertEqual(outcome.diagnostics, [])

    def test_patch_staging_charges_the_total_at_the_directive(self) -> None:
        big = "x" * 10_000
        source = "a = 0\n" + "".join(
            f'@patch("k{index}", "{big}")\n' for index in range(20)
        )
        outcome = evaluate_source(
            source,
            "patch-total",
            Engine(budget=Budget(max_total_value_size=50_000)),
        )
        self.assertEqual(records(outcome), [("X006", None)])


class EnvelopeModeTests(unittest.TestCase):
    def test_e036_is_a_data_mode_surface(self) -> None:
        envelope = (
            'envelope = { version = "1.0.0", tainted = false, payload = {} }\n'
            "x = env.Y\n"
        )
        config = evaluate_source(envelope, "e036-config", Engine(env={"Y": "1"}))
        self.assertEqual(config.diagnostics, [])
        data = evaluate_source(
            'active_mode = "data"\n' + envelope,
            "e036-data",
            Engine(env={"Y": "1"}),
        )
        self.assertEqual(records(data), [("E036", None)])

    def test_envelope_refusals_are_fatal(self) -> None:
        outcome = evaluate_source(
            'active_mode = "data"\nactive_capabilities = [1]\n', "e025"
        )
        self.assertIsNone(outcome.value)
        self.assertEqual(records(outcome), [("E025", "active_capabilities")])

    def test_a_derived_material_payload_breaks_a_false_claim(self) -> None:
        source = (
            's = secret("payload-secret")\n'
            'c = s == "x"\n'
            'active_mode = "data"\n'
            'envelope = { version = "1.0.0", tainted = false, payload = { p = c } }\n'
        )
        outcome = evaluate_source(source, "derived-payload")
        self.assertEqual(records(outcome), [("E035", "envelope.tainted")])

    def test_envelope_version_must_be_strict(self) -> None:
        source = (
            'active_mode = "data"\n'
            'envelope = { version = "1.banana", tainted = false, payload = {} }\n'
        )
        outcome = evaluate_source(source, "bad-version")
        self.assertEqual(records(outcome), [("E033", "envelope.version")])

    def test_v2_requires_a_matching_bound_result(self) -> None:
        budget = (
            "budget = { nodes = 100000, depth = 256, array_len = 100000, "
            "keys = 100000, wall_ms = 100, value_size = 10000000, "
            "total_size = 100000000 }"
        )
        missing = (
            'active_mode = "data"\n'
            'envelope = { version = "2.0.0", tainted = false, payload = { ok = true }, '
            f'receipt = {{ outcome = "clean", mode = "data", {budget} }} }}\n'
        )
        # The source gate refuses a clean v2 envelope with no `result` before
        # evaluation runs (E042), matching the shipped engine; the coalesced
        # post-evaluation E044 bound check covers a present-but-wrong digest.
        outcome = evaluate_source(missing, "missing-result")
        self.assertEqual(records(outcome), [("E042", "envelope.result")])
        wrong = (
            'active_mode = "data"\n'
            'envelope = { version = "2.0.0", profile = "data", tainted = false, '
            'payload = { ok = true }, result = "sha256:' + "0" * 64 + '", '
            f'receipt = {{ outcome = "clean", result = "sha256:{"0" * 64}", '
            f'mode = "data", {budget} }} }}\n'
        )
        outcome = evaluate_source(wrong, "wrong-result")
        self.assertEqual(records(outcome), [("E044", "envelope.result")])

    def test_sender_shape_is_e043(self) -> None:
        source = (
            'active_mode = "data"\n'
            'envelope = { version = "2.0.0", tainted = false, payload = {}, '
            'receipt = { outcome = "failed", failure_class = "other" }, '
            'sender = { id = "" } }\n'
        )
        outcome = evaluate_source(source, "bad-sender")
        self.assertEqual(records(outcome), [("E043", "envelope.sender.id")])

    def test_a_default_filled_payload_is_checked_after_transforms(self) -> None:
        source = (
            'active_mode = "data"\n'
            'schema = { envelope = Table(default = { version = "1.0.0", '
            'tainted = false, payload = secret("payload-secret") }) }\n'
        )
        outcome = evaluate_source(source, "default-taint")
        self.assertIsNone(outcome.value)
        self.assertIn(outcome.diagnostics[0].code, ("E035", "E044"))

    def test_a_substring_copy_in_the_payload_breaks_a_false_claim(self) -> None:
        source = (
            's = secret("payload-secret")\n'
            'active_mode = "data"\n'
            'envelope = { version = "1.0.0", tainted = false, '
            'payload = { p = "x-payload-secret-y" } }\n'
        )
        outcome = evaluate_source(source, "substring-payload")
        self.assertEqual(records(outcome), [("E035", "envelope.tainted")])


class PatchTests(unittest.TestCase):
    def test_dotted_rename_moves_the_leaf(self) -> None:
        outcome = evaluate_source('a = { b = 1 }\n@rename("a.b", "c")\n', "rename")
        self.assertEqual(outcome.diagnostics, [])
        self.assertEqual(to_json_value(outcome.value), {"a": {"c": 1}})

    def test_metadata_rewrites_are_e019(self) -> None:
        patched = evaluate_source(
            '@patch("active_mode", "data")\nactive_mode = "config"\n', "meta-patch"
        )
        self.assertEqual(records(patched), [("E019", None)])

    def test_an_ai_immutable_overlap_is_e019(self) -> None:
        source = '# @ai_immutable: true\na = 1\n@patch("a", 2)\n'
        outcome = evaluate_source(source, "immutable")
        self.assertEqual(records(outcome), [("E019", None)])

    def test_schema_rewrites_and_dotted_rename_targets_are_e019(self) -> None:
        schema = '@patch("schema.x", 1)\nschema = { x = Int }\n'
        self.assertEqual(records(evaluate_source(schema, "schema")), [("E019", None)])
        dotted = 'a = 1\n@rename("a", "b.c")\n'
        self.assertEqual(records(evaluate_source(dotted, "dotted")), [("E019", None)])

    def test_a_malformed_patch_path_is_e019_not_a_crash(self) -> None:
        outcome = evaluate_source('a = 1\n@patch("a[x]", 2)\n', "bad-path")
        self.assertEqual(records(outcome), [("E019", None)])

    def test_patch_paths_validate_keys_and_indices(self) -> None:
        for path in ("a-b", "123", "a[-1]", "a[", ""):
            source = f'a = 1\n@patch("{path}", 2)\n'
            outcome = evaluate_source(source, f"bad-{path}")
            self.assertEqual(records(outcome), [("E019", None)], path)
        multi = 'a = [[1]]\n@patch("a[0][0]", 2)\n'
        outcome = evaluate_source(multi, "multi-index")
        self.assertEqual(outcome.diagnostics, [])
        self.assertEqual(to_json_value(outcome.value), {"a": [[2]]})


class ProfileAndCheckTests(unittest.TestCase):
    def test_an_unselected_profile_still_evaluates(self) -> None:
        source = 'a = 1\nprofile "p" {\n  x = 1 / 0\n}\n'
        outcome = evaluate_source(source, "unselected-profile")
        self.assertEqual(records(outcome), [("T001", None)])
        self.assertIsNone(outcome.value)

    def test_checks_see_the_patched_final_tree(self) -> None:
        source = (
            "schema {\n"
            "  a = Int\n"
            "  check {\n"
            "    positive = a > 0\n"
            "  }\n"
            "}\n"
            "a = 1\n"
            '@patch("a", -1)\n'
        )
        outcome = evaluate_source(source, "check-final")
        self.assertEqual(records(outcome), [("T009", None)])

    def test_constraints_use_value_and_root_keys(self) -> None:
        failing = evaluate_source(
            "# @constraint: value > 10\nx = 5\n", "constraint-fail"
        )
        self.assertEqual(records(failing), [("T006", "x")])
        passing = evaluate_source(
            "# @constraint: value > 10\nx = 15\n", "constraint-pass"
        )
        self.assertEqual(passing.diagnostics, [])
        rooted = evaluate_source("# @constraint: x > 10\nx = 5\n", "constraint-root")
        self.assertEqual(records(rooted), [("T006", "x")])

    def test_a_multibyte_secret_is_redacted_by_byte_length(self) -> None:
        source = 's = secret("日本語")\nt = "x日本語y"\n'
        outcome = evaluate_source(source, "multibyte")
        value = to_json_value(outcome.value, seeds=outcome.seeds)
        self.assertEqual(value["t"], "x***SECRET***y")


class IncludeLimitTests(unittest.TestCase):
    def test_the_thirty_third_include_is_e016(self) -> None:
        files = {}
        for index in range(34):
            nxt = f'include "f{index + 1}.active"\n' if index < 33 else ""
            files[f"f{index}.active"] = nxt
        files["main.active"] = 'include "f0.active"\n'
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        for name, text in files.items():
            (root / name).write_text(text, encoding="utf-8")
        outcome = evaluate_file(str(root / "main.active"))
        self.assertIn("E016", [d.code for d in outcome.diagnostics])

    def test_a_malformed_pin_is_e015_and_never_merges(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        (root / "lib.active").write_text("a = 1\n", encoding="utf-8")
        for digits in (63, 65):
            (root / "main.active").write_text(
                f'include "lib.active" sha256:{"0" * digits}\n',
                encoding="utf-8",
            )
            outcome = evaluate_file(str(root / "main.active"))
            self.assertEqual(records(outcome), [("E015", None)], digits)
            self.assertIsNone(outcome.value)

    def test_included_metadata_stays_local_and_immutability_travels(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        (root / "meta.active").write_text(
            'active_mode = "data"\nactive_capabilities = ["eval"]\n', encoding="utf-8"
        )
        (root / "main.active").write_text(
            'include "meta.active"\ny = 2\n', encoding="utf-8"
        )
        outcome = evaluate_file(str(root / "main.active"))
        self.assertEqual(outcome.diagnostics, [])
        self.assertEqual(to_json_value(outcome.value), {"y": 2})
        (root / "locked.active").write_text(
            "# @ai_immutable: true\nk = 1\n", encoding="utf-8"
        )
        (root / "main2.active").write_text(
            'include "locked.active"\n@patch("k", 9)\n', encoding="utf-8"
        )
        locked = evaluate_file(str(root / "main2.active"))
        self.assertIn("E019", [d.code for d in locked.diagnostics])

    def test_a_binary_include_is_e017(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        (root / "bin.active").write_bytes(b"\xff\xfe\x00\x01")
        (root / "main.active").write_text('include "bin.active"\n', encoding="utf-8")
        outcome = evaluate_file(str(root / "main.active"))
        self.assertIn("E017", [d.code for d in outcome.diagnostics])

    def test_a_drive_relative_name_is_e017_not_an_escape(self) -> None:
        outcome = evaluate_source('include "a:b"\n', "drive-relative")
        self.assertEqual(records(outcome), [("E017", None)])

    def test_a_5mib_include_is_admitted(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        (root / "big.active").write_text(
            "#" + "x" * (5 * 1024 * 1024) + "\nx = 1\n", encoding="utf-8"
        )
        (root / "main.active").write_text('include "big.active"\n', encoding="utf-8")
        outcome = evaluate_file(str(root / "main.active"))
        self.assertEqual(outcome.diagnostics, [])
        self.assertEqual(to_json_value(outcome.value), {"x": 1})


class RequiresTests(unittest.TestCase):
    def test_dotted_requires_targets_the_root(self) -> None:
        source = (
            "schema {\n"
            "  config = Table { flag = Bool }\n"
            '  user = String(requires = "config.flag")\n'
            "}\n"
            "config = { flag = true }\n"
        )
        outcome = evaluate_source(source, "dotted-requires")
        self.assertEqual(records(outcome), [("T004", "user")])
        self.assertEqual(to_json_value(outcome.value), {"config": {"flag": True}})

    def test_dotted_requires_accepts_a_present_target(self) -> None:
        source = (
            "schema {\n"
            "  config = Table { flag = Bool }\n"
            '  user = String(requires = "config.flag")\n'
            "}\n"
            'config = { flag = true }\nuser = "u"\n'
        )
        outcome = evaluate_source(source, "dotted-requires-present")
        self.assertEqual(outcome.diagnostics, [])


class ExportTests(unittest.TestCase):
    def test_content_hash_matches_the_shipped_lockfile(self) -> None:
        # `active lock` at c394f62 for `a = 1.0 / 10000000.0` (1e-7).
        outcome = evaluate_source("a = 1.0 / 10000000.0\n", "hash-float")
        self.assertEqual(
            content_hash(outcome.value, seeds=outcome.seeds),
            "sha256:b248271d18d09a840564eafaaeb935f11a4d44d6d90ac5b3baa9714420782816",
        )

    def test_redacted_secret_derived_hash_matches_the_lockfile(self) -> None:
        source = (
            's = secret("payload-secret")\n'
            'c = s == "x"\n'
            't = "v=${c}"\n'
            'b = blob("text/plain", t)\n'
        )
        outcome = evaluate_source(source, "hash-secret")
        self.assertEqual(
            content_hash(outcome.value, seeds=outcome.seeds),
            "sha256:bf108917571841a8f0d56b44162d714fc8b0f1f083f7790077fecf3592bb8b25",
        )

    def test_positive_exponent_hash_matches_the_lockfile(self) -> None:
        # `active lock` at c394f62 for 1e21.
        outcome = evaluate_source(
            "a = 1000000000000000000000.0\n", "hash-positive-exponent"
        )
        self.assertEqual(
            content_hash(outcome.value, seeds=outcome.seeds),
            "sha256:a18fbc0bcef4f91fedea4a6ff70089101462846a93a65795324a14b10cb5c4d9",
        )

    def test_float_spelling_matches_the_shipped_writer(self) -> None:
        # Ground truth from `active eval --format json` at c394f62.
        cases = {
            1e21: "1e+21",
            1e-7: "1e-7",
            -0.0: "0.0",
            2.0: "2.0",
            1e15: "1000000000000000.0",
            1e16: "1e+16",
            1e17: "1e+17",
            1e-4: "0.0001",
            1e-5: "0.00001",
            1e-6: "1e-6",
            1e-8: "1e-8",
            1.234e-5: "0.00001234",
            1.234e-6: "1.234e-6",
            1.2345678901234568e16: "1.2345678901234568e+16",
            1234567890123456.0: "1234567890123456.0",
            1 / 3: "0.3333333333333333",
            -1e-7: "-1e-7",
        }
        for value, expected in cases.items():
            self.assertEqual(float_text(value), expected, value)

    def test_nonfinite_floats_are_tagged_and_hashed_distinctly(self) -> None:
        self.assertEqual(
            to_json_value(Value.floating(math.inf)), {"$active.float": "inf"}
        )
        self.assertEqual(
            to_json_value(Value.floating(-math.inf)), {"$active.float": "-inf"}
        )
        self.assertEqual(
            to_json_value(Value.floating(math.nan)), {"$active.float": "nan"}
        )
        self.assertNotEqual(
            content_hash(Value.floating(math.inf)),
            content_hash(Value.floating(0.0)),
        )


class ArithmeticTests(unittest.TestCase):
    def test_integer_division_truncates_without_float_rounding(self) -> None:
        # 9007199254740993 is not representable as f64; the result must
        # come from integer truncation, not `int(a / b)`.
        source = "q = 9007199254740993 / 3\n"
        outcome = evaluate_source(source, "huge-div")
        self.assertEqual(outcome.diagnostics, [])
        self.assertEqual(to_json_value(outcome.value), {"q": 3002399751580331})


class BudgetCodeTests(unittest.TestCase):
    def test_value_size_is_x006(self) -> None:
        outcome = evaluate_source(
            'a = "hello"\n', "x006", Engine(budget=Budget(max_value_size=3))
        )
        self.assertEqual(records(outcome), [("X006", None)])

    def test_data_mode_warns_for_array_length_and_keys(self) -> None:
        array = evaluate_source(
            'active_mode = "data"\na = [1, 2, 3, 4]\n',
            "w013-array",
            Engine(budget=Budget(max_array_len=5)),
        )
        self.assertEqual(records(array), [("W013", None)])
        keys = evaluate_source(
            'active_mode = "data"\na = 1\nb = 1\nc = 1\nd = 1\n',
            "w013-keys",
            Engine(budget=Budget(max_keys=5)),
        )
        self.assertEqual(records(keys), [("W013", None)])

    def test_value_size_precedes_depth_in_static_budgets(self) -> None:
        outcome = evaluate_source(
            "a = { b = { c = 1 } }\n",
            "precedence",
            Engine(budget=Budget(max_depth=1, max_value_size=1)),
        )
        self.assertEqual(records(outcome), [("X006", None)])

    def test_shared_aliases_trip_the_bound_instead_of_hanging(self) -> None:
        source = 'a0 = "xxxxxxxxxx"\n'
        for level in range(1, 21):
            source += f'a{level} = "${{a{level - 1}}}${{a{level - 1}}}"\n'
        outcome = evaluate_source(
            source,
            "blowup",
            Engine(budget=Budget(max_value_size=1000)),
        )
        self.assertEqual(records(outcome), [("X006", None)])


class PatternDialectTests(unittest.TestCase):
    def test_rust_regex_dialect_refuses_backreferences_and_lookaround(self) -> None:
        for pattern in ("(a)\\\\1", "(?=x)", "(?<!x)"):
            source = f'schema {{\n  s = String(pattern = "{pattern}")\n}}\ns = "aa"\n'
            outcome = evaluate_source(source, "pattern")
            self.assertEqual(records(outcome), [("T003", "s")], pattern)


class CycleAndBudgetTests(unittest.TestCase):
    def test_an_unselected_profile_participates_in_the_cycle_check(self) -> None:
        source = 'a = 1\nprofile "p" {\n  x = y\n  y = x\n}\nb = 2\n'
        outcome = evaluate_source(source, "profile-cycle")
        self.assertEqual(records(outcome), [("S001", None)])

    def test_depth_budget_is_x003(self) -> None:
        source = "a = { b = { c = { d = 1 } } }\n"
        outcome = evaluate_source(source, "depth", Engine(budget=Budget(max_depth=1)))
        self.assertEqual(records(outcome), [("X003", None)])


class SchemaMetadataTests(unittest.TestCase):
    def test_semver_is_strict_and_fails_closed(self) -> None:
        self.assertEqual(_semver("1.2.3"), (1, 2, 3))
        for bad in ("1.2", "1.2.3.4", "1.x.3", "", "1..3", "1.2.3-rc1"):
            self.assertIsNone(_semver(bad), bad)
        self.assertTrue(_version_at_least("1.10.0", "1.9.9"))
        self.assertFalse(_version_at_least("1.4.0", "9.0.0"))
        self.assertFalse(_version_at_least("1.4.0", "not-a-version"))

    def test_min_reader_rejects_a_malformed_version(self) -> None:
        source = 'schema { min_reader = "9.x.0"\na = Int\n}\na = 1\n'
        outcome = evaluate_source(source, "bad-min-reader")
        self.assertEqual(records(outcome), [("E020", None)])

    def test_min_reader_floor_is_t007(self) -> None:
        source = 'schema { min_reader = "9.0.0"\na = Int\n}\na = 1\n'
        outcome = evaluate_source(source, "floor", Engine(reader_version="1.4.0"))
        self.assertEqual(records(outcome), [("T007", None)])

    def test_removed_in_requires_a_version_string(self) -> None:
        valid = 'schema {\na = Int(removed_in = "1.0.0")\n}\na = 1\n'
        outcome = evaluate_source(valid, "removed", Engine(reader_version="1.4.0"))
        self.assertEqual(records(outcome), [("T008", "a.a")])
        malformed = 'schema {\na = Int(removed_in = "1.x")\n}\na = 1\n'
        outcome = evaluate_source(malformed, "removed-bad")
        self.assertEqual(records(outcome), [("E020", None)])


class IncludeModuleTests(unittest.TestCase):
    def _project(self, files: dict[str, str]):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        for name, text in files.items():
            (root / name).write_text(text, encoding="utf-8")
        return root

    def test_included_salt_matches_the_shipped_identity(self) -> None:
        root = self._project(
            {
                "lib.active": "y = 2\n",
                "main.active": (
                    'include "lib.active"\n'
                    's = secret("payload-secret")\n'
                    'b = blob("text/plain", "payload-secret")\n'
                ),
            }
        )
        outcome = evaluate_file(str(root / "main.active"))
        self.assertEqual(
            to_json_value(outcome.value)["b"]["$active.blob"]["hash"],
            "blake3:beeee5e4319b605b0e43a721405c31bc52571043f2abf47b5d2cc2eccd61c095",
        )

    def test_included_schema_applies_to_supplied_keys(self) -> None:
        root = self._project(
            {
                "lib.active": "schema {\n  a = Int\n}\n",
                "main.active": 'include "lib.active"\na = "x"\n',
            }
        )
        outcome = evaluate_file(str(root / "main.active"))
        self.assertIn(("T002", "a"), records(outcome))

    def test_included_patch_applies(self) -> None:
        root = self._project(
            {
                "lib.active": '@patch("a", 5)\na = 1\n',
                "main.active": 'include "lib.active"\n',
            }
        )
        outcome = evaluate_file(str(root / "main.active"))
        self.assertEqual(outcome.diagnostics, [])
        self.assertEqual(to_json_value(outcome.value), {"a": 5})

    def test_private_include_is_referenceable_but_not_exported(self) -> None:
        root = self._project(
            {
                "vals.active": "a = 7\nb = 8\n",
                "main.active": 'include "vals.active" private\nx = a\n',
            }
        )
        outcome = evaluate_file(str(root / "main.active"))
        self.assertEqual(outcome.diagnostics, [])
        self.assertEqual(to_json_value(outcome.value), {"x": 7})

    def test_an_aliased_include_schema_reports_under_the_alias(self) -> None:
        root = self._project(
            {
                "lib.active": "schema {\n  a = Int\n}\n",
                "main.active": 'include "lib.active" as lib\nx = lib.a\n',
            }
        )
        outcome = evaluate_file(str(root / "main.active"))
        self.assertIn(("W001", "lib.a"), records(outcome))
        self.assertIn(("T004", "lib.a"), records(outcome))


if __name__ == "__main__":
    unittest.main()

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
import re
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
            "lint",
            "convert",
            "graph",
            "generate",
            "schema-introspect",
            "explain",
            "patch",
            "capability",
            "repair",
            "compatibility",
            "value-inspect",
            "debug",
            "viz",
            "budget",
            "mode",
            "doc",
            "schema-query",
        ):
            self.assertIn(required, names, f"tool surface missing {required}")
        self.assertEqual(len(names), 20, f"tool surface changed: {names}")

    def test_sandbox_profiles_match_code(self) -> None:
        # The descriptor's profiles section and the core's sandbox
        # table are one contract in two renderings: names, budgets,
        # and tool sets must agree exactly.
        profiles = self.doc.get("profiles", {})
        self.assertEqual(
            sorted(profiles),
            ["agent", "data", "strict", "trusted"],
            f"profile set changed: {sorted(profiles)}",
        )
        for name, entry in profiles.items():
            self.assertIn(
                entry.get("budget"),
                ("strict", "interactive", "trusted", "data"),
                f"{name}: unknown budget profile",
            )
            self.assertTrue(entry.get("tools"), f"{name}: profile needs a tool list")
        # Agent is the strict superset of data; trusted matches agent.
        self.assertTrue(
            set(profiles["data"]["tools"]) <= set(profiles["agent"]["tools"]),
            "agent must offer everything data offers",
        )
        self.assertEqual(
            sorted(profiles["agent"]["tools"]),
            sorted(profiles["trusted"]["tools"]),
            "trusted matches the agent tool surface",
        )

    def test_output_descriptions_cover_formats_and_query_shapes(self) -> None:
        tools = {t.get("name"): t for t in self.doc.get("tools", [])}
        eval_output = json.dumps(tools["eval"].get("output", {})).lower()
        for fmt in ("json", "yaml", "toml"):
            self.assertIn(fmt, eval_output, f"eval output omits {fmt}")
        graph_output = json.dumps(tools["graph"].get("output", {})).lower()
        for shape in ("path list", "shortest-path", "content-addressed-above-size"):
            self.assertIn(shape, graph_output, f"graph output omits {shape}")

    def test_codegen_targets_and_features(self) -> None:
        # Every shipped target is discoverable, and each has a feature
        # flag: the Rust target was gate-tested but unlisted before.
        tools = {t.get("name"): t for t in self.doc.get("tools", [])}
        target = tools["generate"]["input"]["target"]
        for name in ("jsonschema", "openapi", "typescript", "go", "rust", "python"):
            self.assertIn(name, target, f"generate input omits {name}")
        features = self.doc.get("features", [])
        for feature in (
            "codegen-jsonschema",
            "codegen-typescript-zod",
            "codegen-go",
            "codegen-rust",
            "codegen-python",
        ):
            self.assertIn(feature, features, f"feature list omits {feature}")

    def test_every_tool_described(self) -> None:
        for tool in self.doc.get("tools", []):
            self.assertTrue(tool.get("name"), "nameless tool")
            self.assertTrue(
                tool.get("description"), f"{tool.get('name')}: no description"
            )

    def test_entry_commands_name_their_flags(self) -> None:
        # The descriptor is the authority agents read; an entry that omits
        # a shipped flag sends them to a command that rejects it. Pin the
        # flags on the entries where drift is most likely.
        entries = {t["name"]: t.get("entry", "") for t in self.doc["tools"]}
        for name in ("check", "format", "convert"):
            self.assertTrue(entries.get(name), f"{name}: entry must be present")
        for flag in ("--profile", "--trace", "--budget-profile"):
            self.assertIn(flag, entries["check"], f"check entry lost {flag}")
        for flag in ("--style", "--write", "--check"):
            self.assertIn(flag, entries["format"], f"format entry lost {flag}")
        self.assertIn("--to", entries["convert"], "convert entry lost --to")
        for tool in self.doc["tools"]:
            self.assertTrue(tool.get("description"), f"{tool['name']}: description")

    def test_entry_flags_exist_in_the_cli_help(self) -> None:
        # When the CLI is built, every --flag named by an entry must appear
        # in that command's help. Skips openly when the binary is absent
        # (the pinned assertions above still run).
        import subprocess

        binary = ROOT / "target" / "debug" / "active.exe"
        if not binary.exists():
            binary = ROOT / "target" / "debug" / "active"
        if not binary.exists():
            self.skipTest("active binary not built")
        # Descriptor names are agent-facing; several map to a differently
        # named CLI subcommand. Entries with their own subcommands (graph,
        # schema, debug, ...) are covered by the pinned assertions above;
        # this live check covers the flat commands.
        cli = {
            "format": "fmt",
            "schema-introspect": "inspect",
            "value-inspect": "inspect",
            "schema-query": "schema",
        }
        flat = ("check", "format", "convert", "lint", "eval", "capability")
        by_name = {t["name"]: t for t in self.doc["tools"]}
        for name in flat:
            tool = by_name[name]
            command = cli.get(name, name)
            help_text = subprocess.run(
                [str(binary), command, "--help"],
                cwd=ROOT,
                capture_output=True,
                text=True,
            ).stdout
            for token in (
                tool.get("entry", "")
                .replace("[", " ")
                .replace("]", " ")
                .replace("|", " ")
                .split()
            ):
                if token.startswith("--"):
                    # Word boundary: `--to` must not match `--toml`.
                    pattern = r"(?<![\w-])" + re.escape(token) + r"(?![\w-])"
                    self.assertRegex(
                        help_text,
                        pattern,
                        f"{tool['name']}: {token} not in --help",
                    )


if __name__ == "__main__":
    unittest.main(verbosity=2)

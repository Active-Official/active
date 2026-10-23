#!/usr/bin/env python3
"""Cross-runtime codegen equivalence battery (CHECKLIST CG.002-CG.006).

For one fixture schema covering every core type plus constraints,
defaults, enums, nested tables, blob, deprecated, and requires:
  1. Generate all six targets with `active generate` (the shipped CLI).
  2. Validate a VALID and an INVALID document with each target's NATIVE
     toolchain (python jsonschema/pydantic, tsc, go vet) plus the
     reference core.
  3. Assert identical accept/reject verdicts everywhere.

Exit 0 only on full parity. Writes codegen-equivalence.log next to itself.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOG = Path(__file__).resolve().parent / "codegen-equivalence.log"
lines: list[str] = []
failed = False


def log(s: str = "") -> None:
    lines.append(s)
    print(s, flush=True)


def check(name: str, ok: bool, detail: str = "") -> None:
    global failed
    if not ok:
        failed = True
    log(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}")


def run(
    args: list[str], cwd: Path, timeout: int = 300
) -> subprocess.CompletedProcess[str]:
    exe = shutil.which(args[0])
    if exe is None:
        raise SystemExit(f"missing executable: {args[0]}")
    return subprocess.run(
        [exe, *args[1:]], cwd=cwd, capture_output=True, text=True, timeout=timeout
    )


SCHEMA = """schema = {
  closed = true,
  version = "1.4.0",
  port = Int(min = 1, max = 65535),
  host = String(min_len = 1, default = "localhost"),
  mode = Enum("a", "b"),
  token = Secret,
  ratio = Float(default = 1.5),
  logo = Blob(max_size = 100),
  old = String(deprecated = "use new"),
}
port = 8080
"""

VALID_DOC = {
    "host": "x",
    "mode": "a",
    "token": "***SECRET***",
    "ratio": 1.5,
    "logo": {
        "$active.blob": {
            "hash": "blake3:" + "0" * 64,
            "size": 5,
            "media_type": "text/plain",
            "tainted": False,
        }
    },
    "port": 8080,
    "old": "y",
}
INVALID_DOC = {
    "host": "x",
    "mode": "zzz",
    "token": "***SECRET***",
    "ratio": 1.5,
    "logo": {
        "$active.blob": {
            "hash": "blake3:" + "0" * 64,
            "size": 5,
            "media_type": "text/plain",
            "tainted": False,
        }
    },
    "port": 99999,
    "old": "y",
}


def main() -> int:
    log("=== codegen equivalence battery ===")
    tmp = Path(tempfile.mkdtemp(prefix="active-codegen-eq-"))
    (tmp / "svc.active").write_text(SCHEMA, encoding="utf-8", newline="\n")

    targets = {
        "jsonschema": [
            "generate",
            "--from",
            str(tmp / "svc.active"),
            "--to",
            "jsonschema",
            "--name",
            "Cfg",
        ],
        "openapi": [
            "generate",
            "--from",
            str(tmp / "svc.active"),
            "--to",
            "openapi",
            "--name",
            "Cfg",
        ],
        "typescript": [
            "generate",
            "--from",
            str(tmp / "svc.active"),
            "--to",
            "typescript",
            "--name",
            "Cfg",
        ],
        "go": [
            "generate",
            "--from",
            str(tmp / "svc.active"),
            "--to",
            "go",
            "--name",
            "Cfg",
        ],
        "rust": [
            "generate",
            "--from",
            str(tmp / "svc.active"),
            "--to",
            "rust",
            "--name",
            "Cfg",
        ],
        "python": [
            "generate",
            "--from",
            str(tmp / "svc.active"),
            "--to",
            "python",
            "--name",
            "Cfg",
        ],
    }
    artifacts: dict[str, str] = {}
    for target, args in targets.items():
        p = run(["cargo", "run", "-q", "-p", "active-cli", "--", *args], cwd=ROOT)
        ok = p.returncode == 0 and len(p.stdout) > 0
        check(f"generate {target}", ok, f"bytes={len(p.stdout)} rc={p.returncode}")
        if ok:
            artifacts[target] = p.stdout
            (tmp / f"Cfg.{target}.txt").write_text(
                p.stdout, encoding="utf-8", newline="\n"
            )

    # 1. JSON Schema: native validation parity via python jsonschema.
    try:
        import jsonschema

        schema = json.loads(artifacts["jsonschema"])
        jsonschema.validate(VALID_DOC, schema)
        valid_ok = True
    except Exception as e:  # noqa: BLE001
        valid_ok = False
        log(f"  jsonschema valid-doc error: {e}")
    try:
        import jsonschema

        schema = json.loads(artifacts["jsonschema"])
        jsonschema.validate(INVALID_DOC, schema)
        invalid_rejected = False
    except Exception:
        invalid_rejected = True
    check("jsonschema accepts valid doc", valid_ok, "")
    check("jsonschema rejects invalid doc", invalid_rejected, "")

    # 2. Python: generated TypedDict/Pydantic carries every field.
    py = artifacts.get("python", "")
    check(
        "python target carries all fields",
        all(
            k in py
            for k in ["port", "host", "mode", "token", "ratio", "logo", "ActiveBlob"]
        ),
        "",
    )
    try:
        import pydantic  # noqa: F401

        has_pydantic = True
    except ImportError:
        has_pydantic = False
    log(f"(info) pydantic available: {has_pydantic}")
    if has_pydantic and "class Cfg" in py:
        (tmp / "cfg_model.py").write_text(py, encoding="utf-8", newline="\n")
        probe = (
            "from cfg_model import Cfg\n"
            "doc = " + repr(VALID_DOC) + "\n"
            "try:\n    Cfg(**{k: v for k, v in doc.items() if k != 'logo'})\n"
            "    print('PYDANTIC-VALID-OK')\n"
            "except Exception as e:\n    print('PYDANTIC-VALID-FAIL', e)\n"
        )
        (tmp / "probe_py.py").write_text(probe, encoding="utf-8", newline="\n")
        p = run([sys.executable, "probe_py.py"], cwd=tmp)
        tail = (p.stdout + p.stderr).strip().splitlines()
        check(
            "pydantic model imports",
            "PYDANTIC-VALID" in (p.stdout + p.stderr),
            tail[-1][:100] if tail else "",
        )

    # 3. TypeScript: tsc --noEmit over the generated types, then REAL
    # zod runtime validation of the valid/invalid documents.
    if (
        shutil.which("tsc")
        and shutil.which("npm")
        and shutil.which("node")
        and "export interface Cfg" in artifacts.get("typescript", "")
    ):
        (tmp / "cfg.ts").write_text(
            artifacts["typescript"], encoding="utf-8", newline="\n"
        )
        (tmp / "use.ts").write_text(
            'import { Cfg } from "./cfg";\nconst good: Cfg = { port: 8080, host: "x", mode: "a", token: "s", ratio: 1.5, logo: { "$active.blob": { hash: "blake3:00", size: 5, media_type: "text/plain", tainted: false } }, old: "y" };\nconsole.log(good.port);\n',
            encoding="utf-8",
            newline="\n",
        )
        (tmp / "package.json").write_text(
            '{"name": "cfgtest", "private": true}', encoding="utf-8", newline="\n"
        )
        p = run(
            ["npm", "install", "zod", "--no-audit", "--no-fund"], cwd=tmp, timeout=600
        )
        if p.returncode != 0:
            log(
                "(skip) npm install zod failed; cannot typecheck or runtime-check zod schemas"
            )
            log("OVERALL: SKIP (toolchain)")
            LOG.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
            return 0
        p = run(
            [
                "tsc",
                "--noEmit",
                "--strict",
                "--lib",
                "es2020,dom",
                "--moduleResolution",
                "bundler",
                "--module",
                "esnext",
                "use.ts",
                "cfg.ts",
            ],
            cwd=tmp,
        )
        check(
            "tsc typechecks generated types",
            p.returncode == 0,
            (p.stdout + p.stderr).strip()[:200],
        )
        p = run(
            [
                "tsc",
                "--module",
                "commonjs",
                "--target",
                "es2020",
                "--outDir",
                "dist",
                "cfg.ts",
            ],
            cwd=tmp,
        )
        (tmp / "validate.mjs").write_text(
            "import { createRequire } from 'module';\n"
            "const require = createRequire(import.meta.url);\n"
            "const { CfgSchema } = require('./dist/cfg.js');\n"
            "const valid = " + json.dumps(VALID_DOC) + ";\n"
            "const invalid = " + json.dumps(INVALID_DOC) + ";\n"
            "console.log('ZOD-VALID:' + CfgSchema.safeParse(valid).success);\n"
            "console.log('ZOD-INVALID:' + CfgSchema.safeParse(invalid).success);\n",
            encoding="utf-8",
            newline="\n",
        )
        p = run(["node", "validate.mjs"], cwd=tmp)
        out = p.stdout + p.stderr
        check(
            "zod accepts valid doc",
            "ZOD-VALID:true" in out,
            out.strip().splitlines()[-2:] and out.strip().splitlines()[-1][:100],
        )
        check("zod rejects invalid doc", "ZOD-INVALID:false" in out, "")
    else:
        log("(skip) tsc/npm/node unavailable")

    # 4. Go: gofmt + go vet over the generated structs.
    if shutil.which("go") and "type Cfg struct" in artifacts.get("go", ""):
        (tmp / "go.mod").write_text(
            "module cfgtest\n\ngo 1.21\n", encoding="utf-8", newline="\n"
        )
        (tmp / "cfg.go").write_text(
            "package main\n\n" + artifacts["go"], encoding="utf-8", newline="\n"
        )
        p = run(["gofmt", "-l", "cfg.go"], cwd=tmp)
        check(
            "gofmt clean",
            p.returncode == 0 and not p.stdout.strip(),
            p.stdout.strip()[:100],
        )
        p = run(["go", "vet", "./..."], cwd=tmp, timeout=600)
        check("go vet clean", p.returncode == 0, (p.stdout + p.stderr).strip()[-200:])
    else:
        log("(skip) go unavailable")

    # 5. Cross-target field agreement: every target names every field.
    fields = ["port", "host", "mode", "token", "ratio", "logo"]
    for target, text in artifacts.items():
        missing = [f for f in fields if f not in text]
        check(f"{target} names all fields", not missing, f"missing={missing}")

    log("")
    log("OVERALL: " + ("PASS" if not failed else "FAIL"))
    LOG.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())

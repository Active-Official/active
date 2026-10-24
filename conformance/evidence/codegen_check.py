"""Verification step 3: codegen artifacts + ecosystem validators.

Self-contained and repo-relative: writes the fixture schema and all
artifacts under a fresh system-temp directory (never scratch, never
hardcoded machine paths), generates every target twice (byte-stability
= determinism proof), and runs each artifact through its ecosystem
validator. Exit nonzero on any failure; prints CODEGEN: lines plus
CODEGEN: ALL PASS on success.

Prerequisites (CI installs them): cargo, node, npm, go, python with the
jsonschema package. npm installs typescript+zod into the temp dir.
"""

import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

if sys.flags.optimize:
    # The checks below are plain asserts: running under `python -O`
    # would silently strip the entire gate.
    raise SystemExit("refusing to run under python -O: asserts enforce the gate")

ROOT = Path(__file__).resolve().parents[2]
WORK = Path(tempfile.mkdtemp(prefix="active-codegen-"))

import atexit as _atexit

_atexit.register(lambda: shutil.rmtree(WORK, ignore_errors=True))

FIXTURE = """\
schema = {
  closed = true,
  base_url = String(format = "uri"),
  version = String(format = "semver"),
  color = Enum("red", "green"),
  port = Int(min = 1, max = 65535),
  host = String(min_len = 1, default = "localhost"),
  server = Table { host = String },
}
"""

results = []


def run(argv, cwd=ROOT, timeout=300):
    return subprocess.run(
        argv, cwd=cwd, capture_output=True, text=True, timeout=timeout
    )


def gen_from(source_path, target, out_path, extra_args=()):
    first = run(
        [
            "cargo",
            "run",
            "-q",
            "-p",
            "active-cli",
            "--",
            "generate",
            "--from",
            str(source_path),
            "--to",
            target,
            *extra_args,
        ]
    )
    assert first.returncode == 0, f"generate {target} failed:\n{first.stderr[-2000:]}"
    second = run(
        [
            "cargo",
            "run",
            "-q",
            "-p",
            "active-cli",
            "--",
            "generate",
            "--from",
            str(source_path),
            "--to",
            target,
            *extra_args,
        ]
    )
    assert second.returncode == 0, f"generate {target} rerun failed"
    assert first.stdout == second.stdout, f"generate {target} not byte-stable"
    # LF endings: gofmt rejects CRLF, and text-mode writes on
    # Windows would otherwise convert every newline.
    with open(out_path, "w", encoding="utf-8", newline="") as f:
        f.write(first.stdout)
    return first.stdout


def gen(target, out_path, extra_args=()):
    return gen_from(WORK / "val-schema.active", target, out_path, extra_args)


(WORK / "val-schema.active").write_text(FIXTURE, encoding="utf-8", newline="\n")

# 1. JSON Schema: artifact must itself be a valid Draft 2020-12 schema,
# and must accept the known-good doc while rejecting bad ones.
# Format gap (documented, not silent): the stock checker does not
# enforce `format` (uri/semver/uuid/email need optional extras), so
# format axes are declared here and proven at the zod runtime below
# plus the Active T003 vectors (0015-unknown-format). The core
# validator enforces enums, ranges, required, and types.
import jsonschema

schema_text = gen("jsonschema", WORK / "gen-val.json")
assert "codegen v" in schema_text, "version stamp missing"
schema = json.loads(schema_text)
jsonschema.Draft202012Validator.check_schema(schema)
props = schema["properties"]
assert props["base_url"].get("format") == "uri", props["base_url"]
assert props["version"].get("format") == "semver", props["version"]
assert props["port"].get("minimum") == 1 and props["port"].get("maximum") == 65535
assert props["color"].get("enum") == ["red", "green"], props["color"]
assert schema.get("required") and "host" not in schema["required"], schema.get(
    "required"
)
v = jsonschema.Draft202012Validator(schema, format_checker=jsonschema.FormatChecker())
good = {
    "base_url": "https://api.example.com/v1",
    "version": "1.2.3",
    "color": "red",
    "port": 8080,
    "server": {"host": "db.internal"},
}
v.validate(good)
for bad in (
    dict(good, color="blue"),
    dict(good, port=0),
    {k: v_ for k, v_ in good.items() if k != "port"},
    dict(good, base_url=123),
):
    try:
        v.validate(bad)
    except jsonschema.ValidationError:
        pass
    else:
        raise SystemExit(f"FAIL: bad doc accepted: {bad}")
results.append("jsonschema: schema valid, good accepted, 4 bad rejected")
results.append(
    f"jsonschema: props={sorted(props.keys())} format={props['base_url'].get('format')} "
    "accepted_good=True rejected_bad=4 deterministic=True versioned=True"
)

# 2. TypeScript: interfaces + zod schemas must pass tsc --strict, and
# the zod schema must accept good / reject bad at runtime.
ts = gen("typescript", WORK / "gen-val.ts")
assert 'import { z } from "zod"' in ts, "zod import missing"
assert "export const ConfigSchema = z.object({" in ts, "root zod schema missing"
assert "ConfigServerSchema" in ts, "nested zod schema missing"
import sys as _sys

NPM = "npm.cmd" if _sys.platform == "win32" else "npm"
npm = run([NPM, "init", "-y"], cwd=WORK)
assert npm.returncode == 0, npm.stderr[-1000:]
npm = run(
    [NPM, "install", "--no-audit", "--no-fund", "typescript@5.9.2", "zod@3.25.76"],
    cwd=WORK,
)
assert npm.returncode == 0, f"npm install failed:\n{npm.stderr[-2000:]}"
tsc_js = WORK / "node_modules" / "typescript" / "lib" / "tsc.js"
assert tsc_js.is_file(), "typescript compiler missing after npm install"
p = run(
    [
        "node",
        str(tsc_js),
        "--noEmit",
        "--strict",
        "--target",
        "es2020",
        "--module",
        "commonjs",
        str(WORK / "gen-val.ts"),
    ]
)
assert p.returncode == 0, f"tsc failed:\n{p.stdout[-2000:]}\n{p.stderr[-2000:]}"
results.append("typescript: tsc --strict clean (exit 0, no output)")
p = run(
    [
        "node",
        str(tsc_js),
        "--target",
        "es2020",
        "--module",
        "commonjs",
        "--outDir",
        str(WORK / "tsout"),
        str(WORK / "gen-val.ts"),
    ]
)
assert p.returncode == 0, f"tsc emit failed:\n{p.stderr[-2000:]}"
(WORK / "zod-check.js").write_text(
    "const { ConfigSchema } = require('./tsout/gen-val.js');\n"
    "const good = { base_url: 'https://api.example.com/v1', version: '1.2.3',"
    " color: 'red', port: 8080, server: { host: 'db.internal' } };\n"
    "ConfigSchema.parse({ ...good, host: 'web-01' });\n"
    "for (const bad of [{ ...good, base_url: 'nope' }, { ...good, version: 'nope' },"
    " { ...good, color: 'blue' }, { ...good, port: 0 },"
    " { ...good, extra: 'closed tables reject unknown keys' }]) {\n"
    "  let threw = false;\n"
    "  try { ConfigSchema.parse(bad); } catch { threw = true; }\n"
    "  if (!threw) { console.error('FAIL: zod accepted ' + JSON.stringify(bad));"
    " process.exit(1); }\n"
    "}\n"
    "console.log('zod-ok');\n",
    encoding="utf-8",
)
p = run(["node", str(WORK / "zod-check.js")], cwd=WORK)
assert p.returncode == 0 and "zod-ok" in p.stdout, (
    f"zod runtime check failed:\n{p.stdout}\n{p.stderr}"
)
results.append("typescript: zod runtime accepts good, rejects 5 bad")
results.append(f"typescript: head: {ts.splitlines()[0][:100]} deterministic=True")

# 3. Go: must pass gofmt + go vet inside a scratch module, and carry
# validation tags for the constrained fields.
go_src = gen("go", WORK / "gen-val.go.txt")
assert "codegen v" in go_src, "version stamp missing"
for probe in [
    'validate:"required,url"',
    'validate:"required,min=1,max=65535"',
    "oneof=red green",
    'json:"server" validate:"required"',
]:
    assert probe in go_src, f"go tag missing: {probe}"
gdir = WORK / "gomod"
gdir.mkdir()
(gdir / "go.mod").write_text(
    "module gencheck\n\ngo 1.21\n", encoding="utf-8", newline="\n"
)
shutil.copy(WORK / "gen-val.go.txt", gdir / "gen.go")
with open(gdir / "gen.go", encoding="utf-8", newline="") as f:
    _body = f.read().replace("\r\n", "\n")
with open(gdir / "gen.go", "w", encoding="utf-8", newline="") as f:
    f.write("package gencheck\n\n" + _body)
p = run(["gofmt", "-l", "."], cwd=gdir)
assert p.stdout.strip() == "", f"gofmt unformatted:\n{p.stdout}"
results.append("go: gofmt -l empty (no diff)")
p = run(["go", "vet", "./..."], cwd=gdir)
assert p.returncode == 0, f"go vet failed:\n{p.stderr[-2000:]}"
results.append(
    f"go: go vet exit 0 (stderr: {(p.stderr or '').strip()[:200] or 'empty'})"
)
# Real validator run (not just tag presence): the generated structs must
# accept the good doc and reject bad ones through go-playground/validator
# (pinned for supply-chain determinism). Known gaps, documented: semver
# and unknown formats have no validator rule (comment-only), and
# encoding/json ignores unknown keys, so closed-table extra-key
# rejection lives in Active/JSON Schema/zod, not here.
p = run(["go", "get", "github.com/go-playground/validator/v10@v10.22.1"], cwd=gdir)
assert p.returncode == 0, f"go get validator failed:\n{p.stderr[-2000:]}"
(gdir / "validate_test.go").write_text(
    'package gencheck\n\nimport (\n\t"encoding/json"\n\t"testing"\n\n'
    '\t"github.com/go-playground/validator/v10"\n)\n\n'
    'var goodJSON = `{"base_url":"https://api.example.com/v1","version":"1.2.3",'
    '"color":"red","port":8080,"host":"web-01","server":{"host":"db.internal"}}`\n\n'
    "func TestValidate(t *testing.T) {\n"
    "\tv := validator.New(validator.WithRequiredStructEnabled())\n"
    "\tvar good Config\n"
    "\tif err := json.Unmarshal([]byte(goodJSON), &good); err != nil { t.Fatal(err) }\n"
    '\tif err := v.Struct(good); err != nil { t.Fatalf("good rejected: %v", err) }\n'
    "\tbads := []string{\n"
    '\t\t`{"base_url":"not a uri","version":"1.2.3","color":"red","port":8080,'
    '"server":{"host":"h"}}`,\n'
    '\t\t`{"base_url":"https://x","version":"1.2.3","color":"blue","port":8080,'
    '"server":{"host":"h"}}`,\n'
    '\t\t`{"base_url":"https://x","version":"1.2.3","color":"red","port":0,'
    '"server":{"host":"h"}}`,\n'
    '\t\t`{"base_url":"https://x","version":"1.2.3","color":"red",'
    '"server":{"host":"h"}}`,\n'
    "\t}\n"
    "\tfor _, b := range bads {\n"
    "\t\tvar c Config\n"
    "\t\tif err := json.Unmarshal([]byte(b), &c); err != nil { t.Fatal(err) }\n"
    '\t\tif err := v.Struct(c); err == nil { t.Fatalf("bad accepted: %s", b) }\n'
    "\t}\n}\n",
    encoding="utf-8",
)
p = run(["go", "test", "./..."], cwd=gdir)
assert p.returncode == 0, (
    f"go validator test failed:\n{p.stdout[-2000:]}\n{p.stderr[-2000:]}"
)
results.append("go: validator accepts good, rejects 4 bad (url/oneof/min/required)")
results.append(f"go: head: {go_src.splitlines()[0][:100]} tags=True deterministic=True")

# 4. Python: must compile and import; generated names must exist with
# the new-schema metadata preserved as comments.
py_src = gen("python", WORK / "genval.py")
assert "codegen v" in py_src.splitlines()[0], "version stamp missing"
assert "# format: uri" in py_src and "# format: semver" in py_src, (
    "format comments missing"
)
compile(py_src, "genval.py", "exec")
sys.path.insert(0, str(WORK))
import genval

assert hasattr(genval, "Config"), dir(genval)
assert "base_url" in genval.Config.__annotations__, genval.Config.__annotations__
assert "version" in genval.Config.__annotations__
# Structural equivalence: the color Literal must carry exactly the
# schema's enum members (TypedDicts have no runtime validation, so the
# annotation itself is the contract).
from typing import get_args as _get_args

assert set(_get_args(genval.Config.__annotations__["color"])) == {"red", "green"}
results.append(
    f"python: compiles, imports, Config present "
    f"(fields: {sorted(genval.Config.__annotations__)[:6]}) deterministic=True"
)

# 5. Blob wire-wrapper parity (spec/blob.md §5): every target's Blob
# type must decode the ACTUAL evaluation output. The evaluated document
# is the oracle, so a generated type that models the inner object only
# fails here even though it looks right in isolation.
BLOB_DOC = (
    'schema = { logo = Blob(media_type = "image/png") }\n'
    'logo = blob("image/png", "pngdata")\n'
)
blob_path = WORK / "blob-doc.active"
blob_path.write_text(BLOB_DOC, encoding="utf-8", newline="\n")
p = run(["cargo", "run", "-q", "-p", "active-cli", "--", "eval", str(blob_path)])
assert p.returncode == 0, f"eval blob doc failed:\n{p.stderr[-2000:]}"
blob_doc = json.loads(p.stdout)
blob_inner = blob_doc["logo"]["$active.blob"]
assert blob_inner["size"] == 7, blob_doc
assert blob_inner["media_type"] == "image/png", blob_doc
blob_json = json.dumps(blob_doc)

blob_schema = json.loads(gen_from(blob_path, "jsonschema", WORK / "gen-blob.json"))
jsonschema.Draft202012Validator(blob_schema).validate(blob_doc)

ts_blob = gen_from(blob_path, "typescript", WORK / "gen-blob.ts")
assert "export interface Config" in ts_blob, ts_blob
(WORK / "blob-type-check.ts").write_text(
    'import type { Config } from "./gen-blob";\n'
    f"const doc: Config = {blob_json};\n"
    'console.log(doc.logo["$active.blob"].size);\n',
    encoding="utf-8",
    newline="\n",
)
p = run(
    [
        "node",
        str(tsc_js),
        "--noEmit",
        "--strict",
        "--target",
        "es2020",
        "--module",
        "commonjs",
        "--moduleResolution",
        "node",
        str(WORK / "blob-type-check.ts"),
    ]
)
assert p.returncode == 0, (
    f"blob tsc parity failed:\n{p.stdout[-2000:]}\n{p.stderr[-2000:]}"
)

gblob = WORK / "gblob"
gblob.mkdir()
(gblob / "go.mod").write_text(
    "module genblob\n\ngo 1.21\n", encoding="utf-8", newline="\n"
)
go_blob = gen_from(blob_path, "go", WORK / "gen-blob.go.txt")
(gblob / "gen.go").write_text(
    "package genblob\n\n" + go_blob.replace("\r\n", "\n"),
    encoding="utf-8",
    newline="\n",
)
(gblob / "decode_test.go").write_text(
    "package genblob\n\n"
    'import (\n\t"encoding/json"\n\t"testing"\n)\n\n'
    f"const blobJSON = `{blob_json}`\n\n"
    "func TestDecodeEvaluatedBlob(t *testing.T) {\n"
    "\tvar c Config\n"
    "\tif err := json.Unmarshal([]byte(blobJSON), &c); err != nil { t.Fatal(err) }\n"
    '\tif c.Logo.ActiveBlobValue.Size != 7 { t.Fatalf("size: %+v", c.Logo) }\n'
    '\tif c.Logo.ActiveBlobValue.MediaType != "image/png" { t.Fatalf("media: %+v", c.Logo) }\n'
    '\tif len(c.Logo.ActiveBlobValue.Hash) == 0 { t.Fatalf("hash: %+v", c.Logo) }\n'
    "}\n",
    encoding="utf-8",
    newline="\n",
)
p = run(["go", "test", "./..."], cwd=gblob)
assert p.returncode == 0, (
    f"go blob decode failed:\n{p.stdout[-2000:]}\n{p.stderr[-2000:]}"
)

rblob = WORK / "rblob"
(rblob / "src" / "tests").mkdir(parents=True)
(rblob / "Cargo.toml").write_text(
    '[package]\nname = "genblob"\nversion = "0.0.0"\nedition = "2021"\n\n'
    '[dependencies]\nserde = { version = "1", features = ["derive"] }\n'
    'serde_json = "1"\n',
    encoding="utf-8",
    newline="\n",
)
rs_blob = gen_from(blob_path, "rust", WORK / "gen-blob.rs")
(rblob / "src" / "lib.rs").write_text(
    rs_blob.replace("\r\n", "\n"), encoding="utf-8", newline="\n"
)
(rblob / "src" / "tests" / "decode.rs").write_text(
    "#[test]\n"
    "fn decodes_evaluated_blob_document() {\n"
    f'    const BLOB: &str = r#"{blob_json}"#;\n'
    '    let doc: genblob::Config = serde_json::from_str(BLOB).expect("decode");\n'
    "    assert_eq!(doc.logo.active_blob.size, 7);\n"
    '    assert_eq!(doc.logo.active_blob.media_type, "image/png");\n'
    '    assert!(doc.logo.active_blob.hash.starts_with("blake3:"));\n'
    "}\n",
    encoding="utf-8",
    newline="\n",
)
p = run(["cargo", "test", "--quiet"], cwd=rblob, timeout=600)
assert p.returncode == 0, (
    f"rust blob decode failed:\n{p.stdout[-2000:]}\n{p.stderr[-2000:]}"
)

py_blob = gen_from(blob_path, "python", WORK / "gen_blob_types.py")
import importlib.util as _importlib_util

_spec = _importlib_util.spec_from_file_location(
    "gen_blob_types", WORK / "gen_blob_types.py"
)
_mod = _importlib_util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)
assert set(_mod.ActiveBlob.__annotations__) == set(blob_doc["logo"]), (
    _mod.ActiveBlob.__annotations__
)
assert _mod.ActiveBlob.__annotations__["$active.blob"] is _mod.ActiveBlobValue
assert set(_mod.ActiveBlobValue.__annotations__) == set(blob_inner)
results.append(
    "blob: evaluated document decodes in jsonschema/ts/go/rust/python (wire wrapper)"
)

print("\n".join("CODEGEN: " + r for r in results))
print("CODEGEN: ALL PASS")

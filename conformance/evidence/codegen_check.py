"""Verification step 3: codegen artifacts + ecosystem validators."""

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(r"F:\QQQ-AUDIT\SOLID")
SCR = Path(r"C:\Users\Usuario\AppData\Local\Temp\grok-goal-af4864ec4acf\implementer")
SRC = SCR / "val-schema.active"

results = []


def run(argv, cwd=ROOT, timeout=300):
    p = subprocess.run(argv, cwd=cwd, capture_output=True, text=True, timeout=timeout)
    return p


def gen(target, out_path):
    p = run(
        [
            "cargo",
            "run",
            "-q",
            "-p",
            "active-cli",
            "--",
            "generate",
            "--from",
            str(SRC),
            "--to",
            target,
        ]
    )
    assert p.returncode == 0, f"generate {target} failed:\n{p.stderr[-2000:]}"
    # LF endings: gofmt rejects CRLF, and text-mode writes on
    # Windows would otherwise convert every newline.
    with open(out_path, "w", encoding="utf-8", newline="") as f:
        f.write(p.stdout)
    return p.stdout


# 1. JSON Schema: artifact must itself be a valid Draft 2020-12 schema,
# and must accept the known-good API doc while rejecting a bad one.
import jsonschema

schema_text = gen("jsonschema", SCR / "gen-val.json")
schema = json.loads(schema_text)
jsonschema.Draft202012Validator.check_schema(schema)
props = schema["properties"]
assert props["base_url"].get("format") == "uri", props["base_url"]
assert "description" in props["base_url"], props["base_url"]
v = jsonschema.Draft202012Validator(schema, format_checker=jsonschema.FormatChecker())
good = {
    "base_url": "https://api.example.com/v1",
    "version": "1.2.3",
    "color": "red",
    "cert": "x",
    "tls": True,
}
v.validate(good)
try:
    v.validate(
        {
            "base_url": "not a uri",
            "version": "1.2.3",
            "color": "blue",
            "cert": "x",
            "tls": True,
        }
    )
    raise SystemExit("FAIL: bad doc accepted")
except jsonschema.ValidationError:
    pass
results.append("jsonschema: schema valid, good accepted, bad rejected")
results.append(
    f"jsonschema: props={sorted(props.keys())} format={props['base_url'].get('format')} accepted_good=True rejected_bad=True"
)

# 2. TypeScript: must pass tsc --strict --noEmit (node runs tsc.js
# directly; .cmd shims don't spawn from Python subprocess on Windows).
ts = gen("typescript", SCR / "gen-val.ts")
tsc_js = (
    Path(os.environ.get("APPDATA", ""))
    / "npm"
    / "node_modules"
    / "typescript"
    / "lib"
    / "tsc.js"
)
assert tsc_js.is_file(), f"typescript compiler not found at {tsc_js}"
p = run(
    [
        "node",
        str(tsc_js),
        "--noEmit",
        "--strict",
        "--target",
        "es2020",
        str(SCR / "gen-val.ts"),
    ]
)
assert p.returncode == 0, f"tsc failed:\n{p.stdout[-2000:]}\n{p.stderr[-2000:]}"
assert "interface Config" in ts or "type Config" in ts, ts[:300]
results.append("typescript: tsc --strict clean (exit 0, no output)")
results.append(f"typescript: head: {ts.splitlines()[0][:100]}")

# 3. Go: must pass gofmt + go vet inside a scratch module.
go_src = gen("go", SCR / "gen-val.go.txt")
gdir = Path(tempfile.mkdtemp(prefix="active-gen-go-"))
(gdir / "go.mod").write_text("module gencheck\n\ngo 1.21\n", newline="\n")
shutil.copy(SCR / "gen-val.go.txt", gdir / "gen.go")
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
results.append(f"go: head: {go_src.splitlines()[0][:100]}")

# 4. Python: must compile and import; generated names must exist.
py_src = gen("python", SCR / "genval.py")
compile(py_src, "genval.py", "exec")
sys.path.insert(0, str(SCR))
import genval

assert hasattr(genval, "Config"), dir(genval)
results.append(
    f"python: compiles, imports, Config present (attrs: {[a for a in dir(genval) if not a.startswith('_')][:8]})"
)

print("\n".join("CODEGEN: " + r for r in results))
print("CODEGEN: ALL PASS")

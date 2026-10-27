# Packaging and install-surface audit (goal section 8.3).
# Run from anywhere: pwsh -NoProfile -File conformance/evidence/packaging_audit.ps1
# Writes conformance/evidence/packaging-audit.log and exits non-zero on any
# failed step. Uses a temporary working directory; it never writes config or
# caches into the repository.
$ErrorActionPreference = "Continue"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$ev = Join-Path $repo "conformance\evidence\packaging-audit.log"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("active-pkg-audit-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force $tmp | Out-Null
Set-Location $repo
$fails = 0

function Say($line) { $line | Tee-Object -Append -FilePath $ev }
function Step($name, $code) {
  Say ("{0}_EXIT={1}" -f $name, $code)
  if ($code -ne 0) { $script:fails++ }
}

"PACKAGING AUDIT on $(& git rev-parse --short HEAD) $((Get-Date).ToString('o'))" | Out-File $ev -Encoding utf8
"==============================================================" | Out-File $ev -Append -Encoding utf8

Say "=== 1. Version coherence ==="
$ws = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value
Say "workspace: $ws"
foreach ($c in @("active", "active-core", "active-cli", "active-fmt", "active-lsp", "active-wasm", "active-ffi")) {
  $v = (Select-String -Path "$repo\$c\Cargo.toml" -Pattern '^version\.workspace = true' | Measure-Object).Count
  Say ("crate {0}: workspace-inherited={1}" -f $c, $v)
  if ($v -ne 1) { $fails++ }
}
$py = (Select-String -Path "$repo\bindings\python\pyproject.toml" -Pattern '^version = "([^"]+)"').Matches.Groups[1].Value
$js = (Select-String -Path "$repo\bindings\js\package.json" -Pattern '"version": "([^"]+)"').Matches.Groups[1].Value
$vs = (Select-String -Path "$repo\editors\vscode\package.json" -Pattern '"version": "([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value
$cap = (Select-String -Path "$repo\agent-kit\capability.schema.json" -Pattern '"version": "([^"]+)"').Matches.Groups[1].Value
Say "python: $py  npm: $js  vscode: $vs  capability: $cap"
foreach ($v in @($py, $js, $vs, $cap)) { if ($v -ne $ws) { Say "VERSION-MISMATCH: $v != $ws"; $fails++ } }
Say ("infinity placeholder: " + ((Select-String -Path "$repo\active-infinity\Cargo.toml" -Pattern '^version = "([^"]+)"').Matches.Groups[1].Value) + " (documented, unpublished)")
Say ("prompt pack: " + ((Select-String -Path "$repo\agent-kit\README.md" -Pattern 'v1\.[0-9]+\.[0-9]+' | Select-Object -First 1).Matches.Value))
Say "VERSION_COHERENCE=OK"

Say "=== 2. Python build (documented: separate sdist and wheel) + twine check ==="
Push-Location "$repo\bindings\python"
python -m build --sdist --outdir "$tmp\dist" 2>&1 | Tee-Object -Append -FilePath $ev | Select-Object -Last 3
Step "BUILD_SDIST" $LASTEXITCODE
python -m build --wheel --outdir "$tmp\dist" 2>&1 | Tee-Object -Append -FilePath $ev | Select-Object -Last 3
Step "BUILD_WHEEL" $LASTEXITCODE
Pop-Location
python -m twine check "$tmp\dist\*" 2>&1 | Tee-Object -Append -FilePath $ev
Step "TWINE" $LASTEXITCODE
Say "--- wheel contents ---"
python -c "import zipfile,glob; f=sorted(glob.glob(r'$tmp\dist\*.whl'))[0]; z=zipfile.ZipFile(f); print(f); [print(' ',n) for n in z.namelist()]" 2>&1 | Tee-Object -Append -FilePath $ev
Step "WHEEL_LIST" $LASTEXITCODE

Say "=== 3. npm file-list review (npm pack --dry-run) ==="
Push-Location "$repo\bindings\js"
npm pack --dry-run --json 2>&1 | Out-File "$tmp\npm-pack.raw" -Encoding utf8
Step "NPMPACK" $LASTEXITCODE
Pop-Location
python -c "import json; t=open(r'$tmp\npm-pack.raw',encoding='utf-8').read(); i=t.index(chr(10)+'['+chr(10)); d=json.loads(t[i:]); p=d[0]; print('name',p['name'],'version',p['version'],'files',len(p['files']),'unpacked',p['unpackedSize']); [print(' ',f['path']) for f in p['files']]" 2>&1 | Tee-Object -Append -FilePath $ev
Step "NPMPACK_LIST" $LASTEXITCODE

Say "=== 4. pip install active-lang==1.4.0 (documented command) ==="
python -m venv "$tmp\venv" 2>&1 | Tee-Object -Append -FilePath $ev
Step "VENV" $LASTEXITCODE
& "$tmp\venv\Scripts\python.exe" -m pip install --disable-pip-version-check --no-cache-dir "active-lang==1.4.0" 2>&1 | Tee-Object -Append -FilePath $ev | Select-Object -Last 3
Step "PIP_INSTALL" $LASTEXITCODE
& "$tmp\venv\Scripts\python.exe" -c "import active, importlib.metadata as m; print('module', active.__file__); print('dist version', m.version('active-lang'))" 2>&1 | Tee-Object -Append -FilePath $ev
Step "PIP_IMPORT" $LASTEXITCODE

Say "=== 5. npm install active-lang@1.4.0 (documented command) ==="
New-Item -ItemType Directory -Force "$tmp\npmproj" | Out-Null
Push-Location "$tmp\npmproj"
npm init -y 2>&1 | Out-Null
npm install --no-audit --no-fund "active-lang@1.4.0" 2>&1 | Tee-Object -Append -FilePath $ev | Select-Object -Last 3
Step "NPM_INSTALL" $LASTEXITCODE
node -e "const a=require('active-lang'); const p=require('active-lang/package.json'); console.log('exports', Object.keys(a).join(','), 'version', p.version)" 2>&1 | Tee-Object -Append -FilePath $ev
Step "NPM_REQUIRE" $LASTEXITCODE
Pop-Location

Say "=== 6. cargo install active-cli --version 1.4.0 (documented command) ==="
cargo install active-cli --version 1.4.0 --locked --root "$tmp\cargo-root" 2>&1 | Tee-Object -Append -FilePath $ev | Select-Object -Last 3
Step "CARGO_INSTALL" $LASTEXITCODE
& "$tmp\cargo-root\bin\active.exe" --version 2>&1 | Tee-Object -Append -FilePath $ev
Step "CARGO_CLI_RUN" $LASTEXITCODE

Say "=== 7. cargo add active@1.4.0 + build (documented command) ==="
cargo new --quiet "$tmp\consumer" 2>&1 | Tee-Object -Append -FilePath $ev
Push-Location "$tmp\consumer"
cargo add "active@1.4.0" 2>&1 | Tee-Object -Append -FilePath $ev | Select-Object -Last 3
Step "CARGO_ADD" $LASTEXITCODE
cargo build 2>&1 | Tee-Object -Append -FilePath $ev | Select-Object -Last 2
Step "CARGO_BUILD" $LASTEXITCODE
Pop-Location

Say "=== SUMMARY ==="
if ($fails -eq 0) { Say "PACKAGING-AUDIT: PASS" } else { Say "PACKAGING-AUDIT: FAIL ($fails steps)"; exit 1 }

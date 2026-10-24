# Playground repair loop (GAP-CLOSURE §2.10): run the shipped CLI through
# generate -> check -> repair plan -> partial apply -> eval -> full repair
# and stop on the first unexpected verdict. Requires `active` on PATH or
# ACTIVE_BIN set.

$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$bin = if ($env:ACTIVE_BIN) { $env:ACTIVE_BIN } else { "active" }
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("active-loop-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $work | Out-Null
try {
    $source = Join-Path $here "broken.active"

    Write-Host "1. generate (jsonschema)"
    & $bin generate --from $source --to jsonschema |
        Set-Content -Path (Join-Path $work "schema.json")
    if ($LASTEXITCODE -ne 0) { throw "generate failed" }

    Write-Host "2. check (expected failure)"
    & $bin check $source
    if ($LASTEXITCODE -eq 0) { throw "check unexpectedly passed" }

    Write-Host "3. repair plan"
    $plan = & $bin repair --from $source --format json | ConvertFrom-Json
    if ($plan.steps.Count -lt 2) { throw "expected a multi-step plan" }

    Write-Host "4. apply one step (partial repair)"
    $partial = Join-Path $work "partial.active"
    $applied = & $bin repair --from $source --apply-step 0 --format json `
        --output $partial 2>$null | ConvertFrom-Json
    $written = Get-Content $partial -Raw
    if ($applied.repaired_source.TrimEnd() -ne $written.TrimEnd()) {
        throw "repaired_source does not match the written file"
    }
    & $bin eval $partial 2>$null | Out-Null
    if ($LASTEXITCODE -eq 0) { throw "one step removed every defect" }

    Write-Host "5. apply the full plan"
    $repaired = Join-Path $work "repaired.active"
    & $bin repair --from $source --output $repaired | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "full repair failed" }

    Write-Host "6. eval the repaired document"
    & $bin eval $repaired | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "repaired document does not evaluate" }
    Write-Host "loop complete: $repaired"
}
finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

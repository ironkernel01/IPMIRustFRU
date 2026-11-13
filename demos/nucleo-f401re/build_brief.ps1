# Generate the customer command appendix and PDF from the editable local catalog.
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
Push-Location $repo
try {
    & python -I tools/sync_demo_commands.py
    if ($LASTEXITCODE -ne 0) { throw 'command appendix generation failed; PDF was not rebuilt' }
    & powershell -NoProfile -ExecutionPolicy Bypass -File Documentation/build_pdf.ps1 `
        -DocumentPath demos/nucleo-f401re/ipmc_product_brief.md
    if ($LASTEXITCODE -ne 0) { throw 'demo brief PDF generation failed' }
} finally {
    Pop-Location
}

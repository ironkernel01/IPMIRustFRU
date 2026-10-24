# build_pdf.ps1 - Markdown to PDF for the documents in this folder (headless Chrome, pdf_generator.md).
#   powershell -NoProfile -ExecutionPolicy Bypass -File Documentation\build_pdf.ps1 [ipmc_software_spec]
# The page is the Markdown between pdf\head.html and pdf\foot.html, rendered in the browser
# (marked, mermaid 10.9.1, WaveDrom) and printed by Chrome. The CDN scripts need internet.
# render.html is assembled next to the .md so the drawing paths (..\diagrams\out\png) resolve,
# and is deleted after the print. Mermaid 10.9.1 shows the label break &#10; as a space, so it
# becomes <br/> inside mermaid fences only; the .md keeps &#10;.
param([string]$Name = 'ipmc_software_spec')
$ErrorActionPreference = 'Stop'

$dir = $PSScriptRoot
$md = Join-Path $dir "$Name.md"
$pdf = Join-Path $dir "$Name.pdf"
$render = Join-Path $dir 'render.html'
$profileDir = Join-Path $env:TEMP 'ipmc_pdf_chrome_profile'

$chrome = @(
    "$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
    "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $chrome) { throw 'Chrome or Edge not found' }

$mdText = Get-Content -Raw -Encoding utf8 $md
if ($mdText.Contains('</script>')) { throw "$md contains </script>, which would end the page source" }
$mdText = [regex]::Replace($mdText, '(?s)(```mermaid.*?```)', { param($m) $m.Value -replace '&#10;', '<br/>' })

# Every drawing the document references must exist: a missing PNG prints as an empty box.
$missing = [regex]::Matches($mdText, '!\[[^\]]*\]\(([^)]+\.png)\)') |
    ForEach-Object { $_.Groups[1].Value } |
    Where-Object { -not (Test-Path (Join-Path $dir $_)) }
if ($missing) { throw "missing drawing(s): $($missing -join ', ')" }

$assembled = (Get-Content -Raw -Encoding utf8 (Join-Path $dir 'pdf\head.html')) + $mdText +
    (Get-Content -Raw -Encoding utf8 (Join-Path $dir 'pdf\foot.html'))
[IO.File]::WriteAllText($render, $assembled, (New-Object Text.UTF8Encoding $false))

if (Test-Path $pdf) { Remove-Item $pdf }
$url = 'file:///' + ($render -replace '\\', '/')
# Chrome logs to stderr; Windows PowerShell turns native stderr into error records, which the
# Stop preference would make fatal, so the call runs under Continue and the PDF check decides.
$ErrorActionPreference = 'Continue'
& $chrome --headless --disable-gpu --no-pdf-header-footer "--user-data-dir=$profileDir" `
    --virtual-time-budget=30000 --run-all-compositor-stages-before-draw `
    "--print-to-pdf=$pdf" $url 2>&1 | Out-Null
$ErrorActionPreference = 'Stop'
for ($i = 0; $i -lt 60 -and -not (Test-Path $pdf); $i++) { Start-Sleep -Milliseconds 500 }
Remove-Item $render
if (-not (Test-Path $pdf)) { throw "Chrome did not write $pdf" }
"wrote $pdf ($((Get-Item $pdf).Length) bytes)"

# build_pdf.ps1 - Markdown to PDF for the documents in this folder (headless Chrome, pdf_generator.md).
#   powershell -NoProfile -ExecutionPolicy Bypass -File Documentation\build_pdf.ps1 [ipmc_software_spec]
#   powershell -File Documentation/build_pdf.ps1 -DocumentPath Documentation/ipmc_product_brief.md
# The page is the Markdown between pdf\head.html and pdf\foot.html, rendered in the browser
# (marked, mermaid 10.9.1, WaveDrom) and printed by Chrome. The CDN scripts need internet.
# render.html is assembled next to the .md so the drawing paths (..\Documentation\diagrams\out\png) resolve,
# and is deleted after the print. Mermaid 10.9.1 shows the label break &#10; as a space, so it
# becomes <br/> inside mermaid fences only; the .md keeps &#10;.
param([string]$Name = 'ipmc_software_spec', [string]$DocumentPath)
$ErrorActionPreference = 'Stop'

$dir = $PSScriptRoot
$md = if ($DocumentPath) { (Resolve-Path -LiteralPath $DocumentPath).Path } else { Join-Path $dir "$Name.md" }
$documentDir = Split-Path -Parent $md
$pdf = [IO.Path]::ChangeExtension($md, '.pdf')
$printPdf = Join-Path $documentDir ('.' + [IO.Path]::GetFileNameWithoutExtension($md) + ".build-$PID.pdf")
$render = Join-Path $documentDir 'render.html'
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
    Where-Object { -not (Test-Path (Join-Path $documentDir $_)) }
if ($missing) { throw "missing drawing(s): $($missing -join ', ')" }

$assembled = (Get-Content -Raw -Encoding utf8 (Join-Path $dir 'pdf\head.html')) + $mdText +
    (Get-Content -Raw -Encoding utf8 (Join-Path $dir 'pdf\foot.html'))
[IO.File]::WriteAllText($render, $assembled, (New-Object Text.UTF8Encoding $false))

$url = 'file:///' + ($render -replace '\\', '/')
# Chrome logs to stderr; Windows PowerShell turns native stderr into error records, which the
# Stop preference would make fatal, so the call runs under Continue and the PDF check decides.
$ErrorActionPreference = 'Continue'
& $chrome --headless --disable-gpu --no-pdf-header-footer "--user-data-dir=$profileDir" `
    --virtual-time-budget=30000 --run-all-compositor-stages-before-draw `
    "--print-to-pdf=$printPdf" $url 2>&1 | Out-Null
$ErrorActionPreference = 'Stop'
for ($i = 0; $i -lt 60 -and -not (Test-Path $printPdf); $i++) { Start-Sleep -Milliseconds 500 }
Remove-Item $render
if (-not (Test-Path $printPdf)) { throw "Chrome did not write $printPdf" }
try {
    Move-Item -LiteralPath $printPdf -Destination $pdf -Force
} catch {
    # An open Windows PDF viewer can lock the destination. Keep the old copy
    # intact and give the user a complete, separately named current revision.
    $updated = Join-Path $documentDir ([IO.Path]::GetFileNameWithoutExtension($md) + '.updated.pdf')
    try {
        Move-Item -LiteralPath $printPdf -Destination $updated -Force
    } catch {
        $updated = Join-Path $documentDir ([IO.Path]::GetFileNameWithoutExtension($md) + ".updated-$PID.pdf")
        Move-Item -LiteralPath $printPdf -Destination $updated
    }
    Write-Host "Original PDF is locked; the current revision is $updated"
    $pdf = $updated
}
"wrote $pdf ($((Get-Item $pdf).Length) bytes)"

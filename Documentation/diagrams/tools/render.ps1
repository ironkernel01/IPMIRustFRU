# Render a built drawing set to PNG sheets for checking and for embedding in documents.
#   powershell -File Documentation\diagrams\tools\render.ps1 Documentation\diagrams\out\<set>.pptx
# Output: Documentation\diagrams\out\png\<set>-<sheet>.png
# Needs LibreOffice (soffice.com) and poppler's pdftoppm on PATH, or the
# Documentation\diagrams\.venv PyMuPDF fallback (tools\pdf2png.py)
# (MSYS2 UCRT64: pacman -S mingw-w64-ucrt-x86_64-poppler).
param([Parameter(Mandatory = $true)][string]$Pptx)
$ErrorActionPreference = 'Stop'

$pptxPath = (Resolve-Path $Pptx).Path
$name = [IO.Path]::GetFileNameWithoutExtension($pptxPath)
$out = Split-Path -Parent $pptxPath
$pngDir = Join-Path $out 'png'
New-Item -ItemType Directory -Force -Path $pngDir | Out-Null
Get-ChildItem -Path $pngDir -Filter "$name-*.png" | Remove-Item

# soffice.com waits for the conversion to finish; soffice.exe returns immediately.
$candidates = @('soffice.com', 'soffice')
if ($env:ProgramFiles) {
    $candidates = @((Join-Path $env:ProgramFiles 'LibreOffice\program\soffice.com')) + $candidates
}
$soffice = $candidates | Where-Object { $_ -and (Get-Command $_ -ErrorAction SilentlyContinue) } | Select-Object -First 1
if (-not $soffice) { throw 'LibreOffice not found (soffice.com)' }
$havePdftoppm = [bool](Get-Command pdftoppm -ErrorAction SilentlyContinue)
$venvPy = Join-Path $PSScriptRoot '..\.venv\Scripts\python.exe'
if (-not $havePdftoppm -and -not (Test-Path $venvPy)) { throw 'pdftoppm not on PATH and Documentation\diagrams\.venv (PyMuPDF) missing' }

& $soffice --headless --convert-to pdf --outdir $out $pptxPath | Out-Null
$pdf = Join-Path $out "$name.pdf"
if (-not (Test-Path $pdf)) { throw "LibreOffice did not produce $pdf" }
if ($havePdftoppm) {
    & pdftoppm -r 110 -png $pdf (Join-Path $pngDir $name)
} else {
    & $venvPy -I (Join-Path $PSScriptRoot 'pdf2png.py') $pdf (Join-Path $pngDir $name)
}
Remove-Item $pdf
Get-ChildItem -Path $pngDir -Filter "$name-*.png" | ForEach-Object { $_.FullName }

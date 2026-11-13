#!/usr/bin/env bash
# Render a built drawing set to PNG sheets for checking and for embedding in documents.
#   Documentation/diagrams/tools/render.sh Documentation/diagrams/out/<set>.pptx
# Output: Documentation/diagrams/out/png/<set>-<sheet>.png  (needs LibreOffice soffice and poppler pdftoppm)
set -euo pipefail
pptx="$1"
name="$(basename "${pptx%.pptx}")"
out="$(dirname "$pptx")"
mkdir -p "$out/png"
rm -f "$out/png/$name"-*.png
soffice --headless --convert-to pdf --outdir "$out" "$pptx" >/dev/null
pdftoppm -r 110 -png "$out/$name.pdf" "$out/png/$name"
rm -f "$out/$name.pdf"
ls "$out/png/$name"-*.png

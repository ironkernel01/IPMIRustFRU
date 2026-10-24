"""PDF to PNG fallback when pdftoppm is absent (PyMuPDF).
Usage: python -I pdf2png.py <in.pdf> <out_prefix>   writes <out_prefix>-<n>.png, n from 1, 110 dpi."""
import sys

import pymupdf

doc = pymupdf.open(sys.argv[1])
width = 1 if len(doc) < 10 else len(str(len(doc)))
for i, page in enumerate(doc, 1):
    page.get_pixmap(dpi=110).save(f"{sys.argv[2]}-{str(i).zfill(width)}.png")

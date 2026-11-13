# Public documentation

| Document | PDF | Source |
|---|---|---|
| Product brief, license/pricing and command matrix | [PDF](ipmc_product_brief.pdf) | [Markdown](ipmc_product_brief.md) |
| Software specification and serial firmware reload | [PDF](ipmc_software_spec.pdf) | [Markdown](ipmc_software_spec.md) |
| Executed test results | [PDF](ipmc_test_coverage.pdf) | [Markdown](ipmc_test_coverage.md) |

The [NUCLEO-F401RE demo](../demos/nucleo-f401re/README.md) holds firmware,
programming instructions and its validation record. Drawings, presentation sources
and rendering tools live in `Documentation/diagrams`; the shared PDF template and
logo live in `Documentation/pdf`. Commands below run from the repository root.

## Rebuild the brief

Maintain `local_only/vita_commmands.md` as the editable VITA 46.11-2022 command
catalog. Its mandatory/optional ratings and implementation columns generate the
brief's full command appendix. The local source stays ignored; the generated
customer appendix is included in the public brief. To update the appendix and PDF:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File Documentation/build_brief.ps1
```

To rebuild drawing assets as well, or check the appendix without writing:

```powershell
python -I tools/sync_demo_commands.py
python -I tools/sync_demo_commands.py --check
node Documentation/diagrams/src/product-brief.js
powershell -NoProfile -ExecutionPolicy Bypass -File Documentation/diagrams/tools/render.ps1 Documentation/diagrams/out/product-brief.pptx
powershell -NoProfile -ExecutionPolicy Bypass -File Documentation/build_pdf.ps1 -DocumentPath Documentation/ipmc_product_brief.md
```

The drawing source is `Documentation/diagrams/src/product-brief.js`; its
presentation and rendered PNG are in `Documentation/diagrams/out`. The PDF command uses the shared document template.

## Regenerate specification and test PDFs

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File Documentation/build_pdf.ps1 ipmc_software_spec
python -I tools/test_coverage.py
python -I tools/publish_conformance_results.py
powershell -NoProfile -ExecutionPolicy Bypass -File Documentation/build_pdf.ps1 ipmc_test_coverage
```

The test report generator runs the host/tool tests and reads the recorded board
results. The publisher keeps full raw requirement reports locally and publishes
executed PASS/FAIL checks. Questions: [support@ironkernel.com](mailto:support@ironkernel.com).

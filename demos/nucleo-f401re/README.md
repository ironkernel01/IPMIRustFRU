# NUCLEO-F401RE Tier-1 demo

Read the [PDF brief](ipmc_product_brief.pdf) for the demonstration, wiring,
license choices and fees. Its editable source is [ipmc_product_brief.md](ipmc_product_brief.md).

| License | One-time fee, USD per product/SKU | Maintenance |
|---|---:|---|
| Tier-1 evaluation or qualifying Open Product | Free under the public license | Free public updates |
| Tier-1 commercial | $5,000 | Free; no annual fee |
| Tier-2/3 commercial (Tier-1 included) | $25,000 | $5,000/year |

No royalties or per-unit fees apply. A paid agreement defines deployment rights,
full-source deliverables and maintenance coverage. Enterprise licensing for multiple
product SKUs is available upon request and quotation. A Tier-1 paid license is credited in full
toward an upgrade for the same product/SKU. Engineering services are
separately quoted. Paid Tier-2/3 customers receive full source for their licensed product/SKU; the
source remains confidential and cannot be publicly redistributed.

The free Open Product option requires complete public Product Design Sources,
offering Changes back within 90 days and the required notices. Read
[LICENSE.md](../../LICENSE.md) for all conditions. Public source access alone
does not permit commercial use outside those terms.

## Build the loadable images

From the repository root:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File demos/nucleo-f401re/package.ps1
```

The command requires the pinned Rust toolchain, GNU Arm binutils and Python used
by the existing firmware build helper. It forces `IPMC_TIER23=0`, builds and checks
fresh Tier-1 images, and exports these files to [images/](images/README.md):

- `ipmc-board-tier1.bin`: combined bootloader and runtime for initial ST-LINK drive loading.
- `ipmc-nucleo-f401re-tier1.hex`: runtime for the UART bootloader update tool.
- `SHA256SUMS.txt`: hashes of both image files.
- `manifest.json`: Tier, source revision, dirty-worktree state, compiler and hashes.

It does not flash a connected board. Its build uses the current `defs.rs` board
configuration; review that configuration and the manifest before distributing a demo.
The script restores the previous build environment on success or failure.

Copy the combined `.bin` to the NUCLEO's ST-LINK programming drive, normally
`NOD_F401RE`. **This erases saved FRU inventory and settings.** Do not copy an
application-only file to that drive. Open the console at 115200 baud, 8N1.
The HEX file is for [fwupdate.py](../../tools/fwupdate/README.md), not drive copying.
See the [pinout](../../Documentation/nucleo_f401re_pinout.md) and
[usage guide](../../Documentation/stm32f401_usage_guide.md) for wiring and diagnostics.

## Rebuild the brief

Maintain `local_only/vita_commmands.md` as the editable VITA 46.11-2022 command
catalog. Its mandatory/optional ratings and implementation columns generate the
brief's full command appendix. The local source stays ignored; the generated
customer appendix is included in the public brief. To update the appendix and PDF:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File demos/nucleo-f401re/build_brief.ps1
```

To rebuild drawing assets as well, or check the appendix without writing:

```powershell
python -I tools/sync_demo_commands.py
python -I tools/sync_demo_commands.py --check
node diagrams/src/product-brief.js
powershell -NoProfile -ExecutionPolicy Bypass -File diagrams/tools/render.ps1 demos/nucleo-f401re/assets/product-brief.pptx
powershell -NoProfile -ExecutionPolicy Bypass -File Documentation/build_pdf.ps1 -DocumentPath demos/nucleo-f401re/ipmc_product_brief.md
```

The drawing source stays in `diagrams/src/product-brief.js`; this demo holds its
presentation and rendered PNG. The PDF command uses the shared document template.

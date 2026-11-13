# NUCLEO-F401RE Tier-1 demo

Read the [PDF brief](../../Documentation/ipmc_product_brief.pdf) for the demonstration, wiring,
license choices and fees. Its editable source is [ipmc_product_brief.md](../../Documentation/ipmc_product_brief.md).
See [validation.md](validation.md) for the current image checks and bench results.

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
The demo brief includes the IPMB-A connection table and console firmware-update
procedure. The detailed engineering and bench documents are kept locally.

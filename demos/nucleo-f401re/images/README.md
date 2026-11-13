# NUCLEO-F401RE Tier-1 images

These files are built from the public Tier-1 software with Rust 1.99.1 and
`IPMC_TIER23=0`. They contain no private Tier-2/3 extension or evaluation timer.
The current firmware advertises both configured IPMB interfaces; physical bench
validation uses one Aardvark on IPMB-A. The recorded physical results are for IPMB-A.

| File | Purpose |
|---|---|
| `ipmc-board-tier1.bin` | Initial ST-LINK drive installation: bootloader and application, 182,272 bytes |
| `ipmc-nucleo-f401re-tier1.hex` | Application reload through the serial bootloader |
| `SHA256SUMS.txt` | SHA-256 digests of the loadable files |
| `manifest.json` | Build source, compiler, configuration digest and image hashes |

The application is 133,120 bytes; the bootloader BIN is 23,360 bytes. Header and
SHA-256 checks pass. Serial reload through the console CLI on COM4 passed in
38.1 seconds with matching digest and `OK RUN`. The ordinary APP update preserves
the bootloader and stored FRU data. The [root README](../../../README.md) and
[demo brief](../../../Documentation/ipmc_product_brief.pdf) describe first load and subsequent reload.

Copy only the combined board BIN to the ST-LINK drive, normally `NOD_F401RE`.
**Initial drive loading erases saved FRU inventory and settings.** Use the HEX
file with `tools/fwupdate/fwupdate.py`; do not drive-copy the application image.

To rebuild, run `demos/nucleo-f401re/package.ps1` from the repository root.
Rebuilding replaces the manifest and requires checking the resulting image hashes
against the tested package before carrying validation evidence forward. No Tier-2/3
images belong in this directory. License terms are in the root `LICENSE.md`.

Executed validation results: [validation.md](../validation.md).

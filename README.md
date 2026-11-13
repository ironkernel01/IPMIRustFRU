# IPMIRustFRU

VITA 46.11-2022 Tier-1 IPMC firmware in Rust, with an STM32 NUCLEO-F401RE
demonstration, bootloader, serial firmware-update tools and PC emulator. The
public release includes firmware update over the console CLI and application UART.
The separately licensed Tier-2/3 extension adds events and further management functions.

**Questions, licensing, enterprise quotes and support:**
[support@ironkernel.com](mailto:support@ironkernel.com).

## License and pricing

The public code uses the [IPMIRustFRU Source-Available License](LICENSE.md).
Prices are USD; paid license fees cover **one product/SKU**. There are no royalties
or per-unit fees. Enterprise licensing for multiple product SKUs is available by quote.

| Offering | One-time license fee | Maintenance |
|---|---:|---|
| Tier-1 evaluation, development, research, teaching and personal use | Free under section 2 | Free public updates |
| Tier-1 qualifying Open Product | Free under section 3 | Free public updates |
| Tier-1 commercial, including products with private Design Sources | **$5,000 per product/SKU** | **Free; no annual fee** |
| Tier-2/3 commercial, Tier-1 included, full source delivered under license | **$25,000 per product/SKU** | **$5,000/year per licensed product/SKU** |
| Enterprise: multiple product SKUs | Contact us for a quote | Defined in the agreement |

The no-fee Open Product option requires publishing the Product's **complete Design
Sources**, including your own parts of the design, under the license's terms;
offering Changes back within 90 days; and retaining the required notices. Read
[LICENSE.md](LICENSE.md) for every condition. Public access alone does not grant
unrestricted commercial use. Products that cannot meet those conditions need a
paid agreement before commercial delivery or deployment.

Tier-2 is not sold separately. Tier-2/3 customers receive full source to build and
modify for the licensed product/SKU; the extension repository and delivered source
remain confidential and cannot be publicly redistributed. The signed commercial
agreement defines rights, deliverables and support. Tier-1 public updates have no
annual charge and do not promise a support response time. A paid Tier-1 license is
credited in full toward a Tier-2/3 upgrade for the same product/SKU. Engineering
services are separately quoted.

## Start with the PDFs

Browse the [documentation index](Documentation/README.md) for the documents and rebuild instructions.

| Document | PDF | Editable source | Contents |
|---|---|---|---|
| NUCLEO demo and product brief | [Download PDF](Documentation/ipmc_product_brief.pdf) | [Markdown](Documentation/ipmc_product_brief.md) | Features, wiring, loading, license choices, prices and the complete VITA command-support matrix |
| Software specification | [Download PDF](Documentation/ipmc_software_spec.pdf) | [Markdown](Documentation/ipmc_software_spec.md) | Architecture, crate responsibilities, commands, interfaces, bootloader, builds and serial firmware reload |
| Test coverage | [Download PDF](Documentation/ipmc_test_coverage.pdf) | [Markdown](Documentation/ipmc_test_coverage.md) | Recorded Rust/Python test results, replay results, hardware evidence and verification limits |

The brief lists mandatory (`M`), optional (`O`), conditionally mandatory (`O/M`)
and not-applicable (`NA`) requirements separately from implemented/not-implemented
status for each Tier. [IPMI/VITA command reference](Documentation/ipmi_commands.md)
provides the detailed source-table references. Tier-2/3 command behavior is described
publicly; its implementation source is supplied only under its commercial agreement.

## Demo firmware and scripts

Use the [NUCLEO-F401RE demo folder](demos/nucleo-f401re/README.md) and read its
[validation record](demos/nucleo-f401re/validation.md) for tested images, results
and the tested configuration.
It is specifically for the STM32F401RE; do not load these files onto another MCU.

| File or tool | Purpose |
|---|---|
| [ipmc-board-tier1.bin](demos/nucleo-f401re/images/ipmc-board-tier1.bin) | Initial installation: combined bootloader and Tier-1 application |
| [ipmc-nucleo-f401re-tier1.hex](demos/nucleo-f401re/images/ipmc-nucleo-f401re-tier1.hex) | Subsequent Tier-1 application reload over serial |
| [SHA256SUMS.txt](demos/nucleo-f401re/images/SHA256SUMS.txt) | Check the image-file hashes |
| [manifest.json](demos/nucleo-f401re/images/manifest.json) | Build provenance, configuration and image hashes |
| [Image notes](demos/nucleo-f401re/images/README.md) | Availability, formats and validation status |
| [package.ps1](demos/nucleo-f401re/package.ps1) | Build and package fresh Tier-1 images without flashing a board |
| [fwupdate.py](tools/fwupdate/fwupdate.py) and [instructions](tools/fwupdate/README.md) | Enter the bootloader, erase the application, load Intel HEX, verify and restart |
| [gen.ps1](crates/ipmc-nucleo-f401re/gen.ps1) | Build, initial flash and console helper |

Download the repository to keep the scripts, configuration and documentation together.
On GitHub, open an image or PDF link and select **Download raw file**. Verify image
hashes against `SHA256SUMS.txt`; a SHA-256 digest checks integrity, not a signature.

### First load through ST-LINK

1. Connect the NUCLEO-F401RE through its ST-LINK USB connector.
2. Verify the combined board image's hash. Copy `ipmc-board-tier1.bin` to the
   ST-LINK programming drive, normally labelled `NOD_F401RE`.
3. Wait for programming and reset; inspect `FAIL.TXT` if programming fails.
4. Open the ST-LINK virtual COM port at **115200 baud, 8N1**. Try `HELP`,
   `SYS STATUS`, `IPMB STATS`, `SENSORS` and `FRU SHOW`.

The combined image programs from `0x08000000` and **erases saved FRU inventory and
settings**. Do not copy the application HEX file or an application-only binary to
the ST-LINK drive. The HEX file belongs to the serial update procedure below.

### Reload firmware through the console CLI

The bootloader and CLI update path are part of the **public Tier-1 software**;
no paid Tier-2/3 extension is needed. Install the Python serial dependency:

```powershell
python -m pip install pyserial
```

Close other programs using the console port. From the repository root, replace
`COM4` with your NUCLEO's actual ST-LINK virtual COM port:

```powershell
python tools/fwupdate/fwupdate.py --port COM4 --uart cli `
  --enter cli --enter-port COM4 update `
  demos/nucleo-f401re/images/ipmc-nucleo-f401re-tier1.hex
```

The tool sends `DEBUG BOOTLOADER` through the running CLI, waits for `BOOT>`,
pre-checks the HEX file, erases APP, loads the image, verifies SHA-256 and runs it.
It asks for confirmation if restarting interrupts payload power. An ordinary APP
update preserves the bootloader and stored FRU data. The runtime starts at
`0x0800C000`. The application UART is also supported with `--uart app --enter app`.

See the [firmware-update guide](tools/fwupdate/README.md) for the manual bootloader
commands, field entry, confirmation, timeouts, recovery and command-line options.
The [software specification](Documentation/ipmc_software_spec.pdf) contains the
complete serial reload procedure and protocol details.

## Build from source

The pinned compiler and targets are in [rust-toolchain.toml](rust-toolchain.toml).
On Windows, install [Rust/rustup](https://rustup.rs/), the GNU Windows host linker,
the `thumbv7em-none-eabihf` target, Python 3 and GNU Arm binutils
(`arm-none-eabi-objcopy` and `arm-none-eabi-size`). Put their executable directories
on PATH. The demo packaging script checks these prerequisites before building.

```powershell
# Public Tier-1 host tests, regardless of whether the private extension is present.
$env:IPMC_TIER23 = '0'
cargo test --workspace

# Build and package the NUCLEO bootloader and Tier-1 runtime; does not flash hardware.
powershell -NoProfile -ExecutionPolicy Bypass -File demos/nucleo-f401re/package.ps1
```

The shared source configuration can select a private extension build. For a public
Tier-1 build, explicitly set `IPMC_TIER23=0` or use `package.ps1`, which forces that
setting and restores the prior environment afterward. Review
[defs.rs](crates/ipmc-defs/src/defs.rs) for board, bus and sensor configuration.

## Test over IPMB-A

Connect Aardvark SCL to **PB8/D15**, SDA to **PB9/D14**, and common ground.
Keep Aardvark target power off. Use suitable I2C pull-ups. One Aardvark tests one bus;
IPMB-B failover and dual-bus hardware behavior need additional equipment.

Install the [Total Phase USB driver](https://www.totalphase.com/products/usb-drivers/)
and the Python bench packages as described in [python-ipmi setup](tools/python-ipmi/README.md).
The [Chassis Manager tool](tools/chassis-manager/README.md) performs discovery,
FRU/SDR reads and sensor polling. [Conformance tests](tools/conformance/REQUIREMENTS.md)
record PASS/FAIL results for executed requirement-level checks.
Use the coverage report to distinguish host results from physical-board results.

## Code and tool documentation

| Area | Start here |
|---|---|
| Architecture and features | [Software specification](Documentation/ipmc_software_spec.md) |
| IPMI/VITA command support | [Command reference](Documentation/ipmi_commands.md) |
| Bootloader and image utilities | [ipmc-boot](crates/ipmc-boot/README.md) |
| Serial application port | [Host client](tools/app-port/README.md), [application library](crates/ipmc-app/README.md) |
| Firmware reload | [fwupdate](tools/fwupdate/README.md) |
| Chassis Manager and Aardvark | [Chassis Manager](tools/chassis-manager/README.md), [python-ipmi](tools/python-ipmi/README.md) |
| IPMB trace capture | [Capture tools](tools/capture/README.md) |
| Persistent FRU log | [ipmc-log](crates/ipmc-log/README.md) |
| Test evidence | [Coverage report](Documentation/ipmc_test_coverage.pdf) |

Questions, product/SKU licenses, enterprise quotes, maintenance and engineering:
[support@ironkernel.com](mailto:support@ironkernel.com).

# Tier-1 NUCLEO-F401RE validation

Validated 11-13-2026 using the packaged Tier-1 images, Rust 1.99.1
(58cf8f9c3), Windows GNU host, GNU Arm binutils 2.47 and Python 3.13.
The Aardvark USB driver package is v4.0.0; API v6.00, python-ipmi 0.6.1,
pyaardvark 0.8.1 and pyserial 3.5. The NUCLEO ST-LINK drive was `NOD_F401RE`
and its virtual COM port was COM4. Port names are host-specific.

## Results

| Check | Result |
|---|---|
| Tier-1 Rust tests | 800 passed, 0 failed |
| Rust tests with private Tier-2/3 extension | 853 passed, 0 failed |
| Python tool tests | 220 passed, 0 failed |
| Host and firmware formatting / Clippy | Passed |
| Duplicate-code gate | 0 failing pairs; 1 non-failing existing warning |
| Tier-1 strict emulator conformance | 103 PASS, 0 FAIL |
| Tier-1 physical IPMB-A conformance | 98 PASS, 0 FAIL |
| Complete physical board report | 160 PASS, 0 FAIL |
| Physical Chassis Manager discovery and polling | 14 checks passed; 228 requests, no retries or timeouts in the initial run |
| Strict emulator Chassis Manager | 15 checks passed; 229 requests, no retries or timeouts |
| Strict emulator vs captured traffic | 0 unexpected differences |
| Capture replay | Tier-1 44/61/115/89; extension 78/64/167/0; both baselines passed |
| Initial programming | Combined board BIN copied through ST-LINK; no FAIL.TXT |
| Serial firmware reload through CLI | Passed, 38.1 s, exit 0, verified image digest, OK RUN |
| Post-reload startup | Tier-1 identity, self-test 55h 00h, correct image length/digest |
| FRU preservation | Decoded Board/Product fields unchanged across reload; persistence write/reset/restore checks passed |

The board report's 160 passes include its requirement checks; do not add the
conformance counts again as independent hardware tests. This customer report lists
executed PASS/FAIL checks. Full raw reports remain local for engineering traceability.

## Tested images

The combined board image is 182,272 bytes; the application image is 133,120 bytes;
the bootloader BIN is 23,360 bytes. Runtime sections: text 132,804, data 184,
BSS 23,164 bytes. Application image SHA-256 verified by the bootloader:

```text
182777d6d3055c3c37b6fa7b1bbd510cee6f933709bd36c8b4069a4f66347503
```

The serial update completed with `OK END 4157 records 133000 bytes, image valid`,
`OK VERIFY valid SHA-256 ... 164 ms`, then `OK RUN`. The host processed 4,161 HEX
records; address/termination records and skipped padding explain the different
programmed-byte and record counts.

Use [SHA256SUMS.txt](images/SHA256SUMS.txt) for file hashes and
[manifest.json](images/manifest.json) for source/configuration provenance. The
bootloader digest covers the application format; file hashes cover the downloadable
BIN and HEX. SHA-256 integrity checking is not a publisher signature.

## Tested configuration

The physical test used a NUCLEO-F401RE and one Aardvark connected to IPMB-A
(PB8 SCL, PB9 SDA and GND), with Aardvark target power off. Serial firmware reload
used the onboard ST-LINK console CLI on COM4. The firmware advertises both
configured IPMB interfaces. Strict emulator tests used both simulated buses with
`--policy alternate --b-power-up strict`, and the capture cross-check passed with
zero unexpected differences. The report presents results for executed checks.

## Operator instructions

See the [root README](../../README.md),
[demo brief](../../Documentation/ipmc_product_brief.pdf),
[software specification](../../Documentation/ipmc_software_spec.pdf) and
[firmware-update guide](../../tools/fwupdate/README.md).
The root README contains license/pricing choices and download links.
Questions and support: [support@ironkernel.com](mailto:support@ironkernel.com).

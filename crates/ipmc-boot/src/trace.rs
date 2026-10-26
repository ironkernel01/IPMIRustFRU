//! trace - the start-up trace lines of design 6.2, one function per line. Each writes the whole
//! line and CR LF through `TextOut`; the labels are padded to 16 columns.

use crate::decision::{Decision, Reason};
use crate::geometry::APP_START;
use crate::header::{ImageError, Report, HEADER_ADDR};
use crate::request::Interface;
use crate::text::{put_addr, put_dec, put_hex, put_hex_bytes, put_version, TextOut, CRLF};
use ipmc_defs::{IPMC_APP_UART_NAME, IPMC_CONSOLE_UART_NAME};

const LABEL_CAUSE: &str = "Reset cause:    ";
const LABEL_PIN: &str = "BOOTLOADER pin: ";
const LABEL_REQUEST: &str = "Request:        ";
const LABEL_IMAGE: &str = "Runtime image:  ";
const LABEL_CHECK: &str = "Image check:    ";
const LABEL_DECISION: &str = "Decision:       ";
const LABEL_CLOCK: &str = "Clock:          ";
const LABEL_STORED: &str = "Stored digest:  ";
const LABEL_BOOTLOADER: &str = "Bootloader:     ";
const LABEL_BUILD: &str = "Build:          ";
const LABEL_PORT: &str = "Port:           ";
const LABEL_ERASED: &str = "Erased:         ";
/// DIGEST_SHOWN -- digest bytes the trace prints (16 hex digits).
const DIGEST_SHOWN: usize = 8;
/// DIGEST_LEN -- bytes of the SHA-256 digest.
const DIGEST_LEN: usize = 32;
/// FLAGS_DIGITS -- hex digits of the flags byte.
const FLAGS_DIGITS: u32 = 2;

/// line -- `label`, the pieces of the line from `body`, then CR LF.
fn line(out: &mut dyn TextOut, label: &str, body: impl FnOnce(&mut dyn TextOut)) {
    out.put(label);
    body(out);
    out.put(CRLF);
}

/// banner -- `ipmc-boot v0.1.0 (sectors 0-1)`, `(sectors 0-1, field)` in the field image.
pub fn banner(out: &mut dyn TextOut, version: &str, field: bool) {
    line(out, "ipmc-boot v", |out| {
        out.put(version);
        out.put(if field {
            " (sectors 0-1, field)"
        } else {
            " (sectors 0-1)"
        });
    });
}

pub fn reset_cause(out: &mut dyn TextOut, text: &str) {
    line(out, LABEL_CAUSE, |out| out.put(text));
}

pub fn pin(out: &mut dyn TextOut, high: bool) {
    line(out, LABEL_PIN, |out| {
        out.put(if high { "1 (forced)" } else { "0" })
    });
}

pub fn request(out: &mut dyn TextOut, request: Option<Interface>) {
    line(out, LABEL_REQUEST, |out| match request {
        None => out.put("none"),
        Some(interface) => {
            out.put("bootloader requested by the runtime firmware (");
            out.put(interface.text());
            out.put(")");
        }
    });
}

/// runtime_image -- found (with version, length and flags) or the reason it is not.
pub fn runtime_image(out: &mut dyn TextOut, report: &Report) {
    line(out, LABEL_IMAGE, |out| {
        out.put("header at ");
        put_addr(out, HEADER_ADDR);
        out.put(": ");
        match (report.result, report.header) {
            (Err(error @ ImageError::BadHeader(field)), _) => {
                out.put(error.name());
                out.put(" (");
                out.put(field.name());
                out.put(")");
            }
            (Err(error @ (ImageError::Missing | ImageError::BadMagic)), _)
            | (Err(error @ ImageError::BadVectorTable), _) => out.put(error.name()),
            (_, Some(header)) => {
                out.put("found, ");
                put_version(out, header.version);
                out.put(", ");
                put_dec(out, header.length);
                out.put(" bytes, flags ");
                put_hex(out, header.flags & 0xFF, FLAGS_DIGITS);
                out.put("h");
            }
            (_, None) => out.put(ImageError::Missing.name()),
        }
    });
}

/// check_line -- the `Image check` line with the first `shown` bytes of the computed digest;
/// `not run` when the header check already failed.
fn check_line(out: &mut dyn TextOut, report: &Report, ms: u32, shown: usize) {
    line(out, LABEL_CHECK, |out| match report.computed {
        None => out.put("not run"),
        Some(computed) => {
            out.put("SHA-256 ");
            put_hex_bytes(out, computed.get(..shown).unwrap_or(&[]));
            out.put(if report.result.is_ok() {
                " match, "
            } else {
                " MISMATCH, "
            });
            put_dec(out, ms);
            out.put(" ms");
        }
    });
}

/// image_check -- the first 8 digest bytes with match or MISMATCH and the time.
pub fn image_check(out: &mut dyn TextOut, report: &Report, ms: u32) {
    check_line(out, report, ms, DIGEST_SHOWN);
}

/// image_check_full -- the same line with all 32 digest bytes (`INFO`).
pub fn image_check_full(out: &mut dyn TextOut, report: &Report, ms: u32) {
    check_line(out, report, ms, DIGEST_LEN);
}

/// digest_or_none -- 64 hex digits, or `none` for a digest that does not exist.
pub fn digest_or_none(out: &mut dyn TextOut, digest: Option<[u8; DIGEST_LEN]>) {
    match digest {
        Some(bytes) => put_hex_bytes(out, &bytes),
        None => out.put("none"),
    }
}

/// stored_digest -- the digest in the image header (all 64 digits), `none` without a header.
pub fn stored_digest(out: &mut dyn TextOut, report: &Report) {
    line(out, LABEL_STORED, |out| {
        digest_or_none(out, report.header.map(|header| header.digest))
    });
}

/// bootloader -- `Bootloader:     ipmc-boot v0.1.0` (`INFO`).
pub fn bootloader(out: &mut dyn TextOut, version: &str) {
    line(out, LABEL_BOOTLOADER, |out| {
        out.put("ipmc-boot v");
        out.put(version);
    });
}

/// build -- `bench` or `field` (`INFO`).
pub fn build(out: &mut dyn TextOut, field: bool) {
    line(out, LABEL_BUILD, |out| {
        out.put(if field { "field" } else { "bench" })
    });
}

/// port -- the session port: `CLI 115200 8N1 RTS/CTS off` (`INFO`).
pub fn port(out: &mut dyn TextOut, name: &str, baud: u32, flow: bool) {
    line(out, LABEL_PORT, |out| {
        out.put(name);
        out.put(" ");
        put_dec(out, baud);
        out.put(if flow {
            " 8N1 RTS/CTS on"
        } else {
            " 8N1 RTS/CTS off"
        });
    });
}

/// erased -- what this session erased: `none`, `FRU`, `APP` or `FRU APP` (`INFO`).
pub fn erased(out: &mut dyn TextOut, fru: bool, app: bool) {
    line(out, LABEL_ERASED, |out| {
        if !fru && !app {
            out.put("none");
        }
        if fru {
            out.put("FRU");
        }
        if fru && app {
            out.put(" ");
        }
        if app {
            out.put("APP");
        }
    });
}

/// ports -- the port names a WAKEUP is accepted on: `CLI or APP`, `CLI` alone without the app port.
fn ports(out: &mut dyn TextOut, app_enabled: bool) {
    out.put(IPMC_CONSOLE_UART_NAME);
    if app_enabled {
        out.put(" or ");
        out.put(IPMC_APP_UART_NAME);
    }
}

pub fn decision(out: &mut dyn TextOut, decision: &Decision, app_enabled: bool) {
    line(out, LABEL_DECISION, |out| match *decision {
        Decision::Start => out.put("starting the runtime"),
        Decision::Countdown(seconds) => {
            out.put("countdown ");
            put_dec(out, seconds);
            out.put(" s for WAKEUP on ");
            ports(out, app_enabled);
        }
        Decision::Stay(Reason::WakeupInCountdown) => {
            out.put("staying in the bootloader: ");
            out.put(Reason::WakeupInCountdown.text());
        }
        Decision::Stay(reason) => {
            out.put("staying in the bootloader: ");
            out.put(reason.text());
            out.put(", waiting for WAKEUP on ");
            ports(out, app_enabled);
        }
    });
}

/// clock_fault -- the clock start-up fault, when there is one (the boot goes on at HSI).
pub fn clock_fault(out: &mut dyn TextOut, text: &str) {
    line(out, LABEL_CLOCK, |out| out.put(text));
}

/// countdown -- the first line of the boot countdown (design 6.5), before the digits:
/// `Send WAKEUP on CLI or APP within 3 s to stay in the bootloader`.
pub fn countdown(out: &mut dyn TextOut, seconds: u32, app_enabled: bool) {
    line(out, "Send WAKEUP on ", |out| {
        ports(out, app_enabled);
        out.put(" within ");
        put_dec(out, seconds);
        out.put(" s to stay in the bootloader");
    });
}

/// starting -- the last line of the start path.
pub fn starting(out: &mut dyn TextOut) {
    line(out, "Starting the runtime firmware at ", |out| {
        put_addr(out, APP_START)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::{decode, Header, HeaderField, HEADER_LEN};
    use std::format;
    use std::string::String;

    fn header(length: u32, flags: u32) -> Header {
        let mut raw = [0u8; HEADER_LEN];
        raw[0..4].copy_from_slice(&crate::header::MAGIC.to_le_bytes());
        raw[8..12].copy_from_slice(&length.to_le_bytes());
        raw[12..16].copy_from_slice(&[0, 1, 0, 0]);
        raw[16..20].copy_from_slice(&flags.to_le_bytes());
        decode(&raw)
    }

    fn report(result: Result<(), ImageError>, with_digest: bool) -> Report {
        let mut digest = [0u8; 32];
        digest[0..8].copy_from_slice(&[0x1F, 0x3A, 0x09, 0xC2, 0x7B, 0x54, 0xE6, 0xD8]);
        Report {
            header: Some(header(111_668, 0)),
            computed: with_digest.then_some(digest),
            result,
        }
    }

    /// text -- the output of one trace function.
    fn text(f: impl FnOnce(&mut String)) -> String {
        let mut out = String::new();
        f(&mut out);
        out
    }

    #[test]
    fn the_example_of_design_6_2_byte_for_byte() {
        let good = report(Ok(()), true);
        let out = text(|out| {
            banner(out, "0.1.0", false);
            reset_cause(out, "power-on");
            pin(out, false);
            request(out, None);
            runtime_image(out, &good);
            image_check(out, &good, 81);
            decision(out, &Decision::Start, true);
            starting(out);
        });
        let want = "ipmc-boot v0.1.0 (sectors 0-1)\r\n\
Reset cause:    power-on\r\n\
BOOTLOADER pin: 0\r\n\
Request:        none\r\n\
Runtime image:  header at 0x0800_C200: found, v0.1.0, 111668 bytes, flags 00h\r\n\
Image check:    SHA-256 1f3a09c27b54e6d8 match, 81 ms\r\n\
Decision:       starting the runtime\r\n\
Starting the runtime firmware at 0x0800_C000\r\n";
        assert_eq!(out, want);
    }

    #[test]
    fn single_line_variants() {
        let mismatch = report(Err(ImageError::DigestMismatch), true);
        let flagged = Report {
            header: Some(header(300, 0x03)),
            computed: None,
            result: Ok(()),
        };
        let rows: [(String, &str); 20] = [
            (text(|o| banner(o, "0.1.0", true)), "ipmc-boot v0.1.0 (sectors 0-1, field)\r\n"),
            (text(|o| pin(o, true)), "BOOTLOADER pin: 1 (forced)\r\n"),
            (
                text(|o| request(o, Some(Interface::Console))),
                "Request:        bootloader requested by the runtime firmware (console)\r\n",
            ),
            (
                text(|o| request(o, Some(Interface::App))),
                "Request:        bootloader requested by the runtime firmware (app port)\r\n",
            ),
            (
                text(|o| runtime_image(o, &Report { header: None, computed: None, result: Err(ImageError::Missing) })),
                "Runtime image:  header at 0x0800_C200: missing\r\n",
            ),
            (
                text(|o| runtime_image(o, &Report { header: None, computed: None, result: Err(ImageError::BadMagic) })),
                "Runtime image:  header at 0x0800_C200: bad magic\r\n",
            ),
            (
                text(|o| runtime_image(o, &Report { header: Some(header(0, 0)), computed: None, result: Err(ImageError::BadHeader(HeaderField::Vectors)) })),
                "Runtime image:  header at 0x0800_C200: bad header (vector address)\r\n",
            ),
            (
                text(|o| runtime_image(o, &Report { header: Some(header(0, 0)), computed: None, result: Err(ImageError::BadVectorTable) })),
                "Runtime image:  header at 0x0800_C200: bad vector table\r\n",
            ),
            (
                text(|o| runtime_image(o, &mismatch)),
                "Runtime image:  header at 0x0800_C200: found, v0.1.0, 111668 bytes, flags 00h\r\n",
            ),
            (
                text(|o| runtime_image(o, &flagged)),
                "Runtime image:  header at 0x0800_C200: found, v0.1.0, 300 bytes, flags 03h\r\n",
            ),
            (
                text(|o| image_check(o, &mismatch, 7)),
                "Image check:    SHA-256 1f3a09c27b54e6d8 MISMATCH, 7 ms\r\n",
            ),
            (
                text(|o| image_check(o, &flagged, 0)),
                "Image check:    not run\r\n",
            ),
            (
                text(|o| decision(o, &Decision::Countdown(3), true)),
                "Decision:       countdown 3 s for WAKEUP on CLI or APP\r\n",
            ),
            (
                text(|o| decision(o, &Decision::Countdown(3), false)),
                "Decision:       countdown 3 s for WAKEUP on CLI\r\n",
            ),
            (
                text(|o| decision(o, &Decision::Stay(Reason::NoImage), true)),
                "Decision:       staying in the bootloader: no image, waiting for WAKEUP on CLI or APP\r\n",
            ),
            (
                text(|o| decision(o, &Decision::Stay(Reason::Pin), false)),
                "Decision:       staying in the bootloader: BOOTLOADER pin, waiting for WAKEUP on CLI\r\n",
            ),
            (
                text(|o| clock_fault(o, "PLL did not lock")),
                "Clock:          PLL did not lock\r\n",
            ),
            (
                text(|o| countdown(o, 3, true)),
                "Send WAKEUP on CLI or APP within 3 s to stay in the bootloader\r\n",
            ),
            (
                text(|o| countdown(o, 30, false)),
                "Send WAKEUP on CLI within 30 s to stay in the bootloader\r\n",
            ),
            (
                text(|o| decision(o, &Decision::Stay(Reason::WakeupInCountdown), true)),
                "Decision:       staying in the bootloader: WAKEUP in countdown\r\n",
            ),
        ];
        for (got, want) in rows {
            assert_eq!(got, want);
        }
    }

    #[test]
    fn info_lines() {
        let mut digest = [0u8; 32];
        digest[0..8].copy_from_slice(&[0x1F, 0x3A, 0x09, 0xC2, 0x7B, 0x54, 0xE6, 0xD8]);
        digest[31] = 0xAB;
        let hex = format!("1f3a09c27b54e6d8{}ab", "00".repeat(23));
        let full = Report {
            header: Some(Header {
                digest,
                ..header(10, 0)
            }),
            computed: Some(digest),
            result: Ok(()),
        };
        let mismatch = Report {
            result: Err(ImageError::DigestMismatch),
            ..full
        };
        let no_header = Report {
            header: None,
            computed: None,
            result: Err(ImageError::Missing),
        };
        let rows: [(String, String); 12] = [
            (
                text(|o| image_check_full(o, &full, 81)),
                format!("Image check:    SHA-256 {hex} match, 81 ms\r\n"),
            ),
            (
                text(|o| image_check_full(o, &mismatch, 5)),
                format!("Image check:    SHA-256 {hex} MISMATCH, 5 ms\r\n"),
            ),
            (
                text(|o| image_check_full(o, &no_header, 0)),
                "Image check:    not run\r\n".into(),
            ),
            (
                text(|o| stored_digest(o, &full)),
                format!("Stored digest:  {hex}\r\n"),
            ),
            (
                text(|o| stored_digest(o, &no_header)),
                "Stored digest:  none\r\n".into(),
            ),
            (
                text(|o| bootloader(o, "0.1.0")),
                "Bootloader:     ipmc-boot v0.1.0\r\n".into(),
            ),
            (
                text(|o| build(o, false)),
                "Build:          bench\r\n".into(),
            ),
            (text(|o| build(o, true)), "Build:          field\r\n".into()),
            (
                text(|o| port(o, "CLI", 115_200, false)),
                "Port:           CLI 115200 8N1 RTS/CTS off\r\n".into(),
            ),
            (
                text(|o| port(o, "APP", 9_600, true)),
                "Port:           APP 9600 8N1 RTS/CTS on\r\n".into(),
            ),
            (
                text(|o| erased(o, true, true)),
                "Erased:         FRU APP\r\n".into(),
            ),
            (
                text(|o| erased(o, false, false)),
                "Erased:         none\r\n".into(),
            ),
        ];
        for (got, want) in rows {
            assert_eq!(got, want);
        }
        assert_eq!(text(|o| erased(o, true, false)), "Erased:         FRU\r\n");
        assert_eq!(text(|o| erased(o, false, true)), "Erased:         APP\r\n");
    }

    #[test]
    fn labels_are_16_columns() {
        for label in [
            LABEL_CAUSE,
            LABEL_PIN,
            LABEL_REQUEST,
            LABEL_IMAGE,
            LABEL_CHECK,
            LABEL_DECISION,
            LABEL_CLOCK,
            LABEL_STORED,
            LABEL_BOOTLOADER,
            LABEL_BUILD,
            LABEL_PORT,
            LABEL_ERASED,
        ] {
            assert_eq!(label.len(), 16, "{label:?}");
        }
    }
}

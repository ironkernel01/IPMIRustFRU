//! fwimage - the host tool of the bootloader build (design 3.5): patch the header of the runtime
//! image, check it with the bootloader's own check, combine the board image, check the size.
//!
//! Exit code 0 ok, 1 refused (one line names the reason), 2 usage.
//!   fwimage patch <app.img> <header.bin>
//!   fwimage check <app.img>
//!   fwimage combine <boot.bin> <app.img> <board.bin>
//!   fwimage size <boot.bin>

use ipmc_boot::geometry::{APP_LEN_MAX, APP_START, BOOT_END, BOOT_START};
use ipmc_boot::text::{put_hex_bytes, TextOut};
use ipmc_boot::trace;
use ipmc_boot::{check, patch, SliceImage};
use std::process::ExitCode;

/// ALIGN -- the image length is padded with FFh to a multiple of this many bytes.
const ALIGN: usize = 4;
/// BOOT_LEN_MAX -- the BOOT region: sector 0 (16 384 bytes).
const BOOT_LEN_MAX: usize = (BOOT_END - BOOT_START) as usize;
/// BOOT_LEN_TARGET -- the design target: 75 % of sector 0 (12 288 bytes).
const BOOT_LEN_TARGET: usize = BOOT_LEN_MAX / 4 * 3;
/// BOARD_APP_AT -- where the APP image starts in the board image.
const BOARD_APP_AT: usize = (APP_START - BOOT_START) as usize;

/// Out -- a TextOut that collects the text.
struct Out(String);

impl TextOut for Out {
    fn put(&mut self, text: &str) {
        self.0.push_str(text);
    }
}

/// pad_to_alignment -- appends FFh until the length is a multiple of ALIGN.
fn pad_to_alignment(image: &mut Vec<u8>) {
    while !image.len().is_multiple_of(ALIGN) {
        image.push(0xFF);
    }
}

/// combine -- the board image: the bootloader at 0, FFh up to the APP start (0x8000), the APP
/// image from there. Refuses a bootloader above the BOOT region and an APP image above the APP
/// region.
fn combine(boot: &[u8], app: &[u8]) -> Result<Vec<u8>, String> {
    if boot.len() > BOOT_LEN_MAX {
        return Err(format!(
            "boot image is {} bytes, above {BOOT_LEN_MAX}",
            boot.len()
        ));
    }
    if app.len() > APP_LEN_MAX as usize {
        return Err(format!(
            "APP image is {} bytes, above {APP_LEN_MAX}",
            app.len()
        ));
    }
    let mut board = boot.to_vec();
    board.resize(BOARD_APP_AT, 0xFF);
    board.extend_from_slice(app);
    Ok(board)
}

/// Size -- the size line and what it means.
struct Size {
    line: String,
    warning: Option<String>,
    fits: bool,
}

/// size_report -- `ipmc-boot.bin: <n> bytes, <p> % of 16384`, a warning above the 12 288-byte
/// target, and whether the image fits the 16 384-byte region.
fn size_report(len: usize) -> Size {
    let percent = (len * 100 + BOOT_LEN_MAX / 2) / BOOT_LEN_MAX;
    Size {
        line: format!("ipmc-boot.bin: {len} bytes, {percent} % of {BOOT_LEN_MAX}"),
        warning: (len > BOOT_LEN_TARGET).then(|| {
            format!("warning: ipmc-boot.bin is above the {BOOT_LEN_TARGET}-byte target (75 % of sector 0)")
        }),
        fits: len <= BOOT_LEN_MAX,
    }
}

/// read -- a file, or the one-line reason.
fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))
}

/// write -- a file, or the one-line reason.
fn write(path: &str, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("cannot write {path}: {e}"))
}

fn run_patch(app: &str, header: &str) -> Result<(), String> {
    let mut image = read(app)?;
    pad_to_alignment(&mut image);
    let bytes = patch(&mut image).map_err(|e| format!("refused: {}", e.name()))?;
    write(app, &image)?;
    write(header, &bytes)?;
    let digest = bytes
        .get(ipmc_boot::header::DIGEST_OFFSET..ipmc_boot::header::DIGEST_OFFSET + 32)
        .unwrap_or(&[]);
    let mut text = Out(String::new());
    put_hex_bytes(&mut text, digest);
    println!("patched: {} bytes, SHA-256 {}", image.len(), text.0);
    Ok(())
}

fn run_check(app: &str) -> Result<(), String> {
    let image = read(app)?;
    let report = check(&SliceImage {
        base: APP_START,
        bytes: &image,
    });
    let mut text = Out(String::new());
    trace::runtime_image(&mut text, &report);
    trace::image_check(&mut text, &report, 0);
    print!("{}", text.0);
    match report.result {
        Ok(()) => Ok(()),
        Err(error) => Err(format!("refused: {}", error.name())),
    }
}

fn run_combine(boot: &str, app: &str, board: &str) -> Result<(), String> {
    let image = combine(&read(boot)?, &read(app)?)?;
    write(board, &image)?;
    println!("combined: {board}, {} bytes", image.len());
    Ok(())
}

fn run_size(boot: &str) -> Result<(), String> {
    let report = size_report(read(boot)?.len());
    println!("{}", report.line);
    if let Some(warning) = report.warning {
        println!("{warning}");
    }
    if report.fits {
        Ok(())
    } else {
        Err(format!("refused: above {BOOT_LEN_MAX} bytes"))
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["patch", app, header] => run_patch(app, header),
        ["check", app] => run_check(app),
        ["combine", boot, app, board] => run_combine(boot, app, board),
        ["size", boot] => run_size(boot),
        _ => {
            eprintln!(
                "usage: fwimage patch <app.img> <header.bin> | check <app.img> | \
                 combine <boot.bin> <app.img> <board.bin> | size <boot.bin>"
            );
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("fwimage: {reason}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combine_places_the_parts_at_0_and_8000_with_ff_between() {
        let board = combine(&[1, 2, 3], &[9, 8]).unwrap();
        assert_eq!(board.len(), 0x8000 + 2);
        assert_eq!(&board[..3], &[1, 2, 3]);
        assert!(board[3..0x8000].iter().all(|&b| b == 0xFF));
        assert_eq!(&board[0x8000..], &[9, 8]);
    }

    #[test]
    fn combine_refuses_oversized_parts_and_takes_the_limits() {
        assert!(combine(&vec![0; BOOT_LEN_MAX], &[]).is_ok());
        assert!(combine(&vec![0; BOOT_LEN_MAX + 1], &[]).is_err());
        assert!(combine(&[], &vec![0; APP_LEN_MAX as usize]).is_ok());
        assert!(combine(&[], &vec![0; APP_LEN_MAX as usize + 1]).is_err());
    }

    #[test]
    fn size_limits() {
        // (length, percent text, warns, fits)
        let rows = [
            (0, "0 %", false, true),
            (12_288, "75 %", false, true),
            (12_289, "75 %", true, true),
            (16_384, "100 %", true, true),
            (16_385, "100 %", true, false),
        ];
        for (len, percent, warns, fits) in rows {
            let report = size_report(len);
            assert_eq!(
                report.line,
                format!("ipmc-boot.bin: {len} bytes, {percent} of 16384")
            );
            assert_eq!(report.warning.is_some(), warns, "{len}");
            assert_eq!(report.fits, fits, "{len}");
        }
    }

    #[test]
    fn padding_reaches_a_multiple_of_four() {
        for (len, want) in [(0, 0), (1, 4), (4, 4), (5, 8), (7, 8)] {
            let mut image = vec![0u8; len];
            pad_to_alignment(&mut image);
            assert_eq!(image.len(), want);
        }
    }
}

// app_header.rs - the runtime image header (Documentation/ipmc_boot_plan.md section 4): 64 bytes
// at 0x0800_C200 (link.x puts the `.app_header` section at `__app_header`, link.x asserts the
// address and the size). The bootloader checks the image against it at every reset.
//
// Ownership: this module owns the `APP_HEADER` static. The layout, the magic and the fields are
// `ipmc_boot::header` (one definition for this static, the bootloader and `fwimage`). The static
// is the placeholder: magic, format, size, version, flags and vector address are filled at
// compile time, the image length and the SHA-256 digest are FFh. `fwimage patch` fills them in
// the built image (gen.ps1), and `arm-none-eabi-objcopy --update-section` writes the result back
// into the ELF. The runtime also reads the header back from flash (`image_line`) for the banner.

use crate::bsp::flash_regs::read_flash;
use ipmc_boot::header::{self, FLAG_EVAL, FLAG_TIER23, HEADER_ADDR, HEADER_LEN};
use ipmc_boot::text::{put_dec, put_hex_bytes, put_version, TextOut};

/// flag -- `bit` when `on`, else 0.
const fn flag(on: bool, bit: u32) -> u32 {
    if on {
        bit
    } else {
        0
    }
}

/// FLAGS -- the build flags of this image (header flag bits 0 and 1).
const FLAGS: u32 = flag(ipmc_defs::TIER23_BUILT, FLAG_TIER23) | flag(cfg!(ipmc_eval), FLAG_EVAL);

/// APP_HEADER -- the header placeholder, in the `.app_header` section.
#[used]
#[link_section = ".app_header"]
static APP_HEADER: [u8; HEADER_LEN] =
    header::placeholder(header::version_bytes(env!("CARGO_PKG_VERSION")), FLAGS);

/// DIGEST_SHOWN -- digest bytes the image line prints (16 hex digits).
const DIGEST_SHOWN: usize = 8;

/// image_line -- `v<x.y.z>, <n> bytes, SHA-256 <first 16 hex digits>` from the header in flash
/// (the text after the `Image:` label of the banner and `SYS VERSION`).
pub fn image_line(out: &mut dyn TextOut) {
    let mut raw = [0u8; HEADER_LEN];
    // The header address and length lie inside the flash, so the read cannot fail; a failure
    // would leave `raw` zero and the line would print an empty header.
    let _ = read_flash(HEADER_ADDR, &mut raw);
    let image = header::decode(&raw);
    put_version(out, image.version);
    out.put(", ");
    put_dec(out, image.length);
    out.put(" bytes, SHA-256 ");
    put_hex_bytes(out, image.digest.get(..DIGEST_SHOWN).unwrap_or(&[]));
}

// data_cmds.rs - console views of the data the IPMI handlers serve: FRU SHOW and FRU DUMP over the
// FRU image, SENSORS SDR over the Device SDR repository, FRU STORE over the flash log state.
// Read-only except FRU RESTORE, which asks ipmb_link.rs to save the built image to the log. The data lives in the
// dispatch context that ipmb_link.rs owns, and these handlers only print it. Decoding comes from
// ipmc_fru::records and ipmc_core::sdr; nothing here parses a byte layout of its own.
//
// Nothing here runs in an interrupt handler.

use ipmc_fru::persist::RECORD_LEN;
use ipmc_fru::records::{self as fru, AreaKind};
use ipmc_menu::{optional, Args, CmdError, CmdResult};

use crate::console::Context;
use crate::fru_log;
use crate::ipmb_link::RestoreError;

/// Labels of the predefined fields, in table order (FRU spec Tables 11-1 and 12-1).
const BOARD_LABELS: [&str; 5] = [
    "Manufacturer",
    "Product name",
    "Serial number",
    "Part number",
    "FRU file ID",
];
const PRODUCT_LABELS: [&str; 7] = [
    "Manufacturer",
    "Product name",
    "Part/model number",
    "Version",
    "Serial number",
    "Asset tag",
    "FRU file ID",
];
/// Bytes per FRU DUMP line: ipmi_defs.h IPMC_FRU_DUMP_BYTES_PER_LINE (u32 there), cast once here.
const DUMP_BYTES_PER_LINE: usize = ipmc_defs::IPMC_FRU_DUMP_BYTES_PER_LINE as usize;

/// print_text -- an ASCII field's bytes; anything outside printable ASCII shows as '.'.
fn print_text(data: &[u8]) {
    for &byte in data {
        let shown = if (0x20..=0x7E).contains(&byte) {
            char::from(byte)
        } else {
            '.'
        };
        out!("{}", shown);
    }
}

/// print_area -- one information area: its offset, length, checksum and every field.
fn print_area(image: &[u8], kind: AreaKind, name: &str, units: u8, labels: &[&str]) {
    let offset = usize::from(units) * fru::AREA_ALIGN;
    let area = match fru::area(image, kind, units) {
        Ok(area) => area,
        Err(e) => {
            out!("{} area at {:02X}h: {}\n", name, offset, e.name());
            return;
        }
    };
    out!(
        "{} area at {:02X}h: {} bytes, checksum {}\n",
        name,
        offset,
        area.bytes.len(),
        if area.checksum_ok { "OK" } else { "BAD" }
    );
    if let Some(minutes) = area.mfg_minutes() {
        out!(
            "  Mfg date/time:     {} minutes since 1996-01-01 00:00\n",
            minutes
        );
    }
    for (index, field) in area.fields().enumerate() {
        let label = labels.get(index).copied().unwrap_or("Custom field");
        out!("  {:<18} ", label);
        if field.data.is_empty() {
            out!("(empty)");
        } else if field.is_ascii() {
            print_text(field.data);
        } else {
            out!(
                "type {} binary, {} bytes",
                field.type_code,
                field.data.len()
            );
        }
        out!("\n");
    }
}

/// cmd_fru_show -- the common header, the board and product areas decoded, with checksums.
pub fn cmd_fru_show(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    let dispatch = ctx.ipmb.dispatch();
    let image = dispatch.fru.bytes();
    out!(
        "FRU image: {} bytes, {} in use, {}, NVMRO {}\n",
        fru::SIZE,
        fru::IMAGE_LEN,
        if ctx.ipmb.fru_log_status().is_some() {
            "kept in the flash log (FRU STORE)"
        } else {
            "RAM only (a write is lost at reset)"
        },
        if dispatch.addr.nvmro_asserted {
            "asserted (writes refused)"
        } else {
            "deasserted"
        }
    );
    let header = match fru::common_header(image) {
        Ok(header) => header,
        Err(e) => {
            out!("Common header: {}\n", e.name());
            return Err(CmdError::Reported);
        }
    };
    out!(
        "Common header: internal {:02X}h, chassis {:02X}h, board {:02X}h, product {:02X}h, multirecord {:02X}h, checksum {}\n",
        header.internal,
        header.chassis,
        header.board,
        header.product,
        header.multirecord,
        if header.checksum_ok { "OK" } else { "BAD" }
    );
    print_area(image, AreaKind::Board, "Board", header.board, &BOARD_LABELS);
    print_area(
        image,
        AreaKind::Product,
        "Product",
        header.product,
        &PRODUCT_LABELS,
    );
    Ok(())
}

/// cmd_fru_dump -- `FRU DUMP {OFFSET} {COUNT}`: hex bytes of the image; the defaults are offset 0
/// and the rest of the area. A range outside the area is refused (Read FRU Data does the same).
pub fn cmd_fru_dump(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    let image = ctx.ipmb.dispatch().fru.bytes();
    let offset = optional(args.u32_at(0))?.unwrap_or(0);
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    let count = match optional(args.u32_at(1))? {
        Some(count) => usize::try_from(count).unwrap_or(usize::MAX),
        None => image.len().saturating_sub(offset),
    };
    let Some(bytes) = offset
        .checked_add(count)
        .and_then(|end| image.get(offset..end))
    else {
        out!(
            "Error: offset {} count {} is outside the {}-byte FRU area\n",
            offset,
            count,
            image.len()
        );
        return Err(CmdError::Reported);
    };
    for (line, chunk) in bytes.chunks(DUMP_BYTES_PER_LINE).enumerate() {
        out!("{:04X}:", offset + line * DUMP_BYTES_PER_LINE);
        for byte in chunk {
            out!(" {:02X}", byte);
        }
        out!("\n");
    }
    Ok(())
}

/// print_last_save -- the time of the last save, or "none since boot".
fn print_last_save(last_save_ms: Option<u32>) {
    match last_save_ms {
        Some(ms) => out!("{} ms after boot", ms),
        None => out!("none since boot"),
    }
}

/// cmd_fru_store -- the flash log state: records in the sector, newest sequence number, bytes
/// free, whether a write waits to be saved, the last save, whether the next save erases the
/// sector first, the save counters.
pub fn cmd_fru_store(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    let Some(status) = ctx.ipmb.fru_log_status() else {
        out!(
            "FRU log: off ({}); the FRU image is RAM only\n",
            if fru_log::ENABLED {
                "did not start, see the boot lines"
            } else {
                "IPMC_FRU_PERSIST 0"
            }
        );
        return Ok(());
    };
    let dirty = ctx.ipmb.dispatch().fru.is_dirty();
    out!(
        "FRU log: {}-byte records, {} in the sector, newest sequence ",
        RECORD_LEN,
        status.records
    );
    match status.newest_seq {
        Some(seq) => out!("{}", seq),
        None => out!("none"),
    }
    out!(
        ", {} bytes free\nFRU log: write pending {}, delay {} ms, last save ",
        status.bytes_free,
        if status.pending || dirty { "yes" } else { "no" },
        ipmc_defs::IPMC_FRU_PERSIST_DELAY_MS
    );
    print_last_save(status.last_save_ms);
    out!(
        ", next save erases {}, saves {}, failures {}",
        if status.erase_pending { "yes" } else { "no" },
        status.saves,
        status.failures
    );
    if let Some(error) = status.last_error {
        out!(" (last: {})", error.name());
    }
    out!("\n");
    Ok(())
}

/// cmd_fru_restore -- saves the built image as a new log record (one record write: a power cut
/// loads the previous image or the built one) and puts it back in the store. Asks nothing and
/// prints what it did; refused while NVMRO is asserted (no non-volatile write, VITA 46.11
/// Rule 8-3).
pub fn cmd_fru_restore(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    match ctx.ipmb.fru_restore() {
        Ok(()) => {
            out!("Built image saved as a new log record and back in the store, self test rerun\n");
            Ok(())
        }
        Err(RestoreError::LogOff) => {
            out!("Error: the flash log is off; FRU RESTORE has no log to write\n");
            Err(CmdError::Reported)
        }
        Err(RestoreError::Nvmro) => {
            out!("Error: NVMRO is asserted; the flash log is not written\n");
            Err(CmdError::Reported)
        }
        Err(RestoreError::Failed(error)) => {
            out!("Error: FRU log write failed: {}\n", error.name());
            Err(CmdError::Reported)
        }
    }
}

/// cmd_sensors_sdr -- the Device SDR repository: one line per record.
pub fn cmd_sensors_sdr(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    let dispatch = ctx.ipmb.dispatch();
    let sdr = &dispatch.sdr;
    out!(
        "Device SDR repository: {} records, {} sensors, owner {:02X}h, latest reservation {}\n",
        sdr.record_count(),
        sdr.sensor_count(),
        dispatch.addr.ipmb_address,
        sdr.reservation()
    );
    for id in 0..sdr.record_count() {
        let Some(info) = u16::try_from(id).ok().and_then(|id| sdr.describe(id)) else {
            continue;
        };
        out!("  record {}: ", info.id);
        match info.sensor {
            Some(sensor) => out!(
                "sensor {} type {:02X}h event/reading {:02X}h",
                sensor.number,
                sensor.sensor_type,
                sensor.event_type
            ),
            None => out!(
                "management controller device locator (type {:02X}h)",
                info.record_type
            ),
        }
        out!(", {} bytes, \"", info.length);
        print_text(info.name);
        out!("\"\n");
    }
    Ok(())
}

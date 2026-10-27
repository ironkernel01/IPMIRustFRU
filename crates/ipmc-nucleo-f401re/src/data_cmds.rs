// data_cmds.rs - console views of the data the IPMI handlers serve: FRU SHOW and FRU DUMP over the
// FRU image, FRU FIELDS and FRU GET over the thirteen FRU fields, SENSORS SDR over the Device SDR
// repository, FRU STORE over the flash log state. FRU SET and FRU RESTORE write: SET goes through
// `IpmbLink::set_unit_field`, the one path the application port uses too (live NVMRO check, the
// store's dirty mark, the delayed flash save), and RESTORE asks ipmb_link.rs to save the built
// image to the log. The data lives in the dispatch context that ipmb_link.rs owns. Decoding,
// field names, lengths, parsing and printing come from ipmc_fru (`FruField`, `unit_field`) and
// ipmc_core::sdr; nothing here parses a byte layout of its own.
//
// Nothing here runs in an interrupt handler.

use ipmc_fru::date::{checked_text_of, DateRange};
use ipmc_fru::persist::RECORD_LEN;
use ipmc_fru::records::{self as fru, AreaKind};
use ipmc_fru::{mfg_date, unit_field, FieldError, FieldValue, FruField};
use ipmc_menu::{optional, Args, CmdError, CmdResult};

use crate::bsp::uart::CONSOLE_TX_USABLE;
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
/// Bytes per FRU DUMP line (defs.rs IPMC_FRU_DUMP_BYTES_PER_LINE).
const DUMP_BYTES_PER_LINE: usize = ipmc_defs::IPMC_FRU_DUMP_BYTES_PER_LINE;

// ---- how much a console command may print ----
//
// Console output waits while the TX ring is full (11.5 bytes per millisecond on the wire), and the
// IPMB service waits with it. A command therefore prints at most what the ring holds,
// `CONSOLE_TX_USABLE` bytes (IPMC_UART_TX_BUF - 1), whatever the FRU image, a define or the bus
// put in front of it; a listing that could be longer prints its first part and says what is left
// (the way to see the rest is the line it prints). Every size below counts a line end as two
// bytes (CR LF) and uses the worst text of each row, worked out beside it. `DEBUG DUMP` is not bound
// here: it is a paced job (`log_cmds.rs`, ipmc-log `DumpJob`). The commands not named here print a fixed
// set of lines of fewer than 128 bytes each.

/// SLACK -- room left in the ring for what is still in it when a command starts (the echo of the
/// typed line, the end of the previous output) and for the error line a command may add.
const SLACK: usize = 256;

/// SHOW_FIXED -- the single lines of FRU SHOW, worst case: the image line (105: `FRU image: 65535
/// bytes, 65535 in use, kept in the flash log (FRU STORE), NVMRO asserted (writes refused)`),
/// the common header (96), two area lines (47 each: `Product area at 7F8h: 2040 bytes,
/// checksum BAD`), the manufacturing date line (144: the standard field form), the MultiRecord
/// header (25) and three "more" lines (64 + 52 + 52) = 632, rounded up to 640.
const SHOW_FIXED: usize = 640;

/// SHOW_FIELD_ROW -- one field row of an area: two blanks, the label padded to 18, a blank and 63
/// bytes of text, CR LF = 85 (a binary field row is shorter).
const SHOW_FIELD_ROW: usize = 85;

/// SHOW_FIELDS_MAX -- field rows FRU SHOW prints per area. The areas of this IPMC have 5 and 7
/// fields; 12 leaves room for custom fields. An area with more prints the rest as one line.
const SHOW_FIELDS_MAX: usize = 12;

/// SHOW_RECORD_ROW -- one MultiRecord: the record row (84: `  Record at FFFFh: type FFh, 255
/// data bytes, header checksum BAD, data checksum BAD`) and, for the Alternate Mfg Date/Time
/// record, its date row (121: the out-of-range form with the checksum note) = 205.
const SHOW_RECORD_ROW: usize = 205;

/// SHOW_RECORDS_MAX -- MultiRecord rows FRU SHOW prints: what the ring holds after the fixed
/// lines, `SHOW_FIELDS_MAX` rows of both areas and `SLACK`. (4095 - 640 - 2 x 12 x 85 - 256) /
/// 205 = 5 with the default ring. A bus-written image can hold a record per 5 bytes of the image;
/// the others are counted in the last line.
const SHOW_RECORDS_MAX: usize = CONSOLE_TX_USABLE
    .saturating_sub(SHOW_FIXED + 2 * SHOW_FIELDS_MAX * SHOW_FIELD_ROW + SLACK)
    / SHOW_RECORD_ROW;
const _: () = assert!(
    SHOW_RECORDS_MAX >= 1,
    "IPMC_UART_TX_BUF is too small for the worst FRU SHOW (data_cmds.rs SHOW_RECORDS_MAX)"
);

/// DUMP_LINE -- one FRU DUMP line: `0000:`, a blank and two hex digits per byte, CR LF.
const DUMP_LINE: usize = 5 + 3 * DUMP_BYTES_PER_LINE + 2;

/// DUMP_BYTES_MAX -- bytes one FRU DUMP prints: whole lines that fit the ring after `SLACK`
/// ((4095 - 256) / 55 = 69 lines of 16 = 1104 bytes with the defaults). The area is
/// IPMC_FRU_SIZE bytes (512 by default, up to 65535), so a longer range prints its first part and
/// the line that names the rest.
const DUMP_BYTES_MAX: usize =
    (CONSOLE_TX_USABLE.saturating_sub(SLACK) / DUMP_LINE) * DUMP_BYTES_PER_LINE;
const _: () = assert!(
    DUMP_BYTES_MAX >= DUMP_BYTES_PER_LINE,
    "IPMC_UART_TX_BUF is too small for one FRU DUMP line"
);

/// FIELDS_ROW -- one row of FRU FIELDS: the name padded to 16, ` max `, the length padded to 2,
/// two blanks, 63 bytes of text, CR LF = 90. There are `FruField::ALL.len()` rows (13).
const FIELDS_ROW: usize = 90;
const _: () = assert!(
    FruField::ALL.len() * FIELDS_ROW + SLACK <= CONSOLE_TX_USABLE,
    "IPMC_UART_TX_BUF is too small for FRU FIELDS (data_cmds.rs FIELDS_ROW)"
);

/// SDR_HEAD, SDR_ROW -- the SENSORS SDR header line (85: owner, counts, reservation) and one
/// record row (`  record 65535: sensor 255 type FFh event/reading FFh, 255 bytes, "` + the ID
/// string of at most 16 bytes + `"`, CR LF = 100, rounded up to 128). The repository holds at
/// most `RECORD_COUNT_MAX` (15 with the analog sensors) records.
const SDR_HEAD: usize = 96;
const SDR_ROW: usize = 128;
const _: () = assert!(
    SDR_HEAD + ipmc_core::sdr::RECORD_COUNT_MAX * SDR_ROW + SLACK <= CONSOLE_TX_USABLE,
    "IPMC_UART_TX_BUF is too small for SENSORS SDR (data_cmds.rs SDR_ROW)"
);

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

/// print_mfg_date -- the manufacturing date line of the board area in FRU SHOW, and which stored
/// form is in use (VITA 46.11 section 10.3.10): a nonzero standard 3-byte field (a Chassis Manager
/// wrote it) is the date and the VITA 34h record is not used; while the field is 000000h
/// (unspecified, what this IPMC writes) the date is in the record (`print_multirecords`). The next
/// SET MFG_DATE writes the field back to 000000h.
fn print_mfg_date(image: &[u8]) {
    let standard = match mfg_date(image) {
        Ok(found) => found.standard,
        Err(error) => {
            out!("  Mfg date/time:     unreadable ({})\n", error.name());
            return;
        }
    };
    if standard == 0 {
        out!("  Mfg date/time:     standard field 000000h (unspecified); the date is in the VITA 34h record\n");
    } else {
        out!(
            "  Mfg date/time:     standard field {:06X}h = {} UTC, in use; the VITA 34h record is not used while the standard field is nonzero\n",
            standard,
            FieldValue::Minutes(standard)
        );
    }
}

/// print_alt_date -- the Alternate Mfg Date/Time line (VITA 34h): the date, `unspecified` for
/// minute 0 and, for a count above the last date this IPMC accepts, the count itself and
/// `unspecified` (a reader reads such a record as no date), never a made-up date. A bad checksum
/// is noted.
fn print_alt_date(minutes: u32, checksums_ok: bool) {
    out!("  Alternate Mfg Date/Time (VITA 34h): ");
    match (minutes, checked_text_of(minutes)) {
        (0, _) => out!("unspecified"),
        (_, Some(text)) => out!("{} UTC", core::str::from_utf8(&text).unwrap_or("")),
        (_, None) => out!("out of range ({:X}h minutes), unspecified", minutes),
    }
    if !checksums_ok {
        out!(" (checksum BAD: reads as unspecified)");
    }
    out!("\n");
}

/// print_multirecords -- the MultiRecord area of FRU SHOW: its offset and the first
/// `SHOW_RECORDS_MAX` records, the Alternate Mfg Date/Time record (VITA 34h) decoded with the date
/// it holds, and one line that counts the records left. A missing area or record and a bad
/// checksum are shown, not failed.
fn print_multirecords(image: &[u8], units: u8) {
    if units == 0 {
        out!("MultiRecord area: absent (Mfg date/time unspecified)\n");
        return;
    }
    out!(
        "MultiRecord area at {:02X}h\n",
        usize::from(units) * fru::AREA_ALIGN
    );
    let mut any = false;
    for record in fru::multirecords(image, units).take(SHOW_RECORDS_MAX) {
        any = true;
        out!(
            "  Record at {:02X}h: type {:02X}h, {} data bytes, header checksum {}, data checksum {}\n",
            record.offset,
            record.type_id,
            record.data.len(),
            if record.header_ok { "OK" } else { "BAD" },
            if record.data_ok { "OK" } else { "BAD" }
        );
        if let Some(minutes) = record.alt_mfg_minutes() {
            print_alt_date(minutes, record.header_ok && record.data_ok);
        }
    }
    let left = fru::multirecords(image, units)
        .skip(SHOW_RECORDS_MAX)
        .count();
    if left > 0 {
        out!(
            "  ... {} more MultiRecord records (FRU DUMP shows the bytes)\n",
            left
        );
    }
    if !any {
        out!("  (no record)\n");
    }
}

/// print_area -- one information area: its offset, length, checksum and its first
/// `SHOW_FIELDS_MAX` fields, then one line that counts the rest.
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
    if kind == AreaKind::Board {
        print_mfg_date(image);
    }
    for (index, field) in area.fields().enumerate().take(SHOW_FIELDS_MAX) {
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
    let left = area.fields().count().saturating_sub(SHOW_FIELDS_MAX);
    if left > 0 {
        out!("  ... {} more fields (FRU DUMP shows the bytes)\n", left);
    }
}

/// cmd_fru_show -- the common header, the board and product areas decoded, with checksums. The
/// output is bound by the ring (`SHOW_FIXED`, `SHOW_FIELDS_MAX`, `SHOW_RECORDS_MAX`).
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
    print_multirecords(image, header.multirecord);
    Ok(())
}

/// cmd_fru_dump -- `FRU DUMP {OFFSET} {COUNT}`: hex bytes of the image; the defaults are offset 0
/// and the rest of the area. A range outside the area is refused (Read FRU Data does the same).
/// At most `DUMP_BYTES_MAX` bytes print; a longer range ends in the line that names the rest.
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
    let shown = bytes.get(..DUMP_BYTES_MAX).unwrap_or(bytes);
    for (line, chunk) in shown.chunks(DUMP_BYTES_PER_LINE).enumerate() {
        out!("{:04X}:", offset + line * DUMP_BYTES_PER_LINE);
        for byte in chunk {
            out!(" {:02X}", byte);
        }
        out!("\n");
    }
    if shown.len() < bytes.len() {
        out!(
            "... {} more bytes (FRU DUMP {} {} shows them)\n",
            bytes.len() - shown.len(),
            offset + shown.len(),
            bytes.len() - shown.len()
        );
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
                "IPMC_FRU_PERSIST false"
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

/// print_value -- the current value of `field`: its text, `(empty)` for the no-data field, the
/// date or `unspecified`, or the name of the error that stops the read.
fn print_value(image: &[u8], field: FruField) {
    match unit_field(image, field) {
        Ok(FieldValue::Text("")) => out!("(empty)"),
        Ok(value) => out!("{}", value),
        Err(error) => out!("({})", error.name()),
    }
}

/// cmd_fru_fields -- `FRU FIELDS`: every field with its maximum length and its current value, in
/// the order of `FruField::ALL`.
pub fn cmd_fru_fields(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    let image = ctx.ipmb.dispatch().fru.bytes();
    for field in FruField::ALL {
        out!("{:<16} max {:<2}  ", field.name(), field.max_len());
        print_value(image, field);
        out!("\n");
    }
    Ok(())
}

/// field_arg -- the field named by parameter 0, or the error line and `CmdError::Reported`.
fn field_arg(args: &Args<'_>) -> Result<FruField, CmdError> {
    let name = args.text_at(0)?;
    FruField::from_name(name).ok_or_else(|| {
        out!("Error: unknown field {} (FRU FIELDS lists them)\n", name);
        CmdError::Reported
    })
}

/// cmd_fru_get -- `FRU GET [FIELD]`: the current value of one field.
pub fn cmd_fru_get(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    let field = field_arg(args)?;
    out!("{:<16} ", field.name());
    print_value(ctx.ipmb.dispatch().fru.bytes(), field);
    out!("\n");
    Ok(())
}

/// VALUE_BUF -- room for the words of a SET value joined by single spaces: the longest FRU text
/// is 63 bytes, so a longer join is `TooLong` whatever the field.
const VALUE_BUF: usize = 64;

/// report_field_error -- the `Error:` line of a refused field write: the field, the `FieldError`
/// name and, where the error has a number or a range, that.
fn report_field_error(field: FruField, error: FieldError) -> CmdError {
    out!("Error: {} {}", field.name(), error.name());
    match error {
        FieldError::TooLong => out!(" (maximum {} bytes)", field.max_len()),
        FieldError::BadDate => out!(" ({})", DateRange),
        _ => {}
    }
    out!("\n");
    CmdError::Reported
}

/// cmd_fru_set -- `FRU SET [FIELD] {VALUE...}`: writes one field through
/// `IpmbLink::set_unit_field`, the path the application port uses. The value is the words after
/// the field name joined by single spaces; no value clears a text field or the date (MFG_DATE
/// `unspecified`); MFG_DATE takes `YYYY-MM-DD HH:MM` (UTC). Answers the value as stored. Refused while NVMRO is asserted.
pub fn cmd_fru_set(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    let field = field_arg(args)?;
    // The words joined by single spaces in a local buffer: the console does not use the
    // reference crate ipmc-app, which a customer edits.
    let mut joined = [0u8; VALUE_BUF];
    let mut len = 0;
    for index in 1..args.len() {
        let word = args.text_at(index)?;
        let space = usize::from(index > 1);
        let end = len + space + word.len();
        let Some(slot) = joined.get_mut(len..end) else {
            return Err(report_field_error(field, FieldError::TooLong));
        };
        let (gap, text) = slot.split_at_mut(space);
        gap.fill(b' ');
        text.copy_from_slice(word.as_bytes());
        len = end;
    }
    let value = joined
        .get(..len)
        .and_then(|bytes| core::str::from_utf8(bytes).ok())
        .unwrap_or("");
    if let Err(error) = ctx.ipmb.set_unit_field(field, value) {
        return Err(report_field_error(field, error));
    }
    out!("OK: {} ", field.name());
    print_value(ctx.ipmb.dispatch().fru.bytes(), field);
    if ctx.ipmb.fru_log_status().is_some() {
        out!("; saved to flash after the save delay (FRU STORE)\n");
    } else {
        out!("; kept in RAM only (the flash log is off)\n");
    }
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
        Err(RestoreError::Busy) => {
            out!("Error: busy (an IPMB request is in progress); nothing erased, try again\n");
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

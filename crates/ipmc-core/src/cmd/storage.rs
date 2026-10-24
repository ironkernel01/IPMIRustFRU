//! cmd/storage.rs - Storage NetFn (0Ah) handlers: the FRU Inventory Device commands for FRU 0.
//!
//! Layouts are IPMI v2.0 rev 1.1 section 34: Table 34-2 Get FRU Inventory Area Info, Table 34-3
//! Read FRU Data, Table 34-4 Write FRU Data. The offsets and counts are in bytes (the device is
//! accessed by bytes: Get FRU Inventory Area Info byte 4 bit 0 = 0). VITA 46.11: FRU Device ID
//! other than 0 -> C9h (Rules 4.1.1-17 and 4.2-3); FRU data is readable whatever the payload
//! power (Rule 8-1); a write while NVMRO is asserted -> 80h and changes nothing (Rules 8-3, 8-5).

use crate::cmd::app::copy_out;
use crate::cmd::check_fru_device_id;
use crate::dispatch::{cc, Context};
use crate::ipmb::Frame;
use crate::selftest;
use ipmc_fru::SIZE;

/// ACCESS_BY_BYTES -- Get FRU Inventory Area Info byte 4 = 00h: accessed by bytes (Table 34-2).
const ACCESS_BY_BYTES: u8 = 0;
/// FRU area size as the u16 of Table 34-2 bytes 2:3; IPMC_FRU_SIZE is guarded to 65535 in ipmc-defs.
const AREA_SIZE: u16 = SIZE as u16;

/// offset_of -- the 16-bit offset of a request, LS byte first (Tables 34-3 and 34-4).
fn offset_of(lo: u8, hi: u8) -> usize {
    usize::from(u16::from_le_bytes([lo, hi]))
}

/// get_fru_inventory_area_info -- Get FRU Inventory Area Info (IPMI 2.0 34.1).
/// Request: FRU Device ID (C7h when the length is not 1; C9h for an ID above 0).
/// Response data: area size LS byte, area size MS byte, access type 00h (bytes).
pub fn get_fru_inventory_area_info(
    _ctx: &mut Context,
    req: &Frame,
    out: &mut [u8],
) -> Result<usize, u8> {
    let [fru_id] = req.data else {
        return Err(cc::REQ_LEN_INVALID);
    };
    check_fru_device_id(*fru_id)?;
    let [lo, hi] = AREA_SIZE.to_le_bytes();
    copy_out(out, &[lo, hi, ACCESS_BY_BYTES])
}

/// read_fru_data -- Read FRU Data (IPMI 2.0 34.2).
/// Request: FRU Device ID, offset LS byte, offset MS byte, count (1-based). C7h for a length other
/// than 4; C9h for an ID above 0, a count of 0 or a range beyond the area; CAh when the count
/// plus the count byte does not fit one response frame.
/// Response data: count returned, then the bytes.
pub fn read_fru_data(ctx: &mut Context, req: &Frame, out: &mut [u8]) -> Result<usize, u8> {
    let [fru_id, lo, hi, count] = req.data else {
        return Err(cc::REQ_LEN_INVALID);
    };
    check_fru_device_id(*fru_id)?;
    let count = usize::from(*count);
    if count == 0 {
        return Err(cc::PARAM_OUT_OF_RANGE);
    }
    let data = ctx
        .fru
        .read(offset_of(*lo, *hi), count)
        .map_err(|_| cc::PARAM_OUT_OF_RANGE)?;
    let (count_slot, body) = out.split_first_mut().ok_or(cc::CANT_RETURN_BYTES)?;
    body.get_mut(..count)
        .ok_or(cc::CANT_RETURN_BYTES)?
        .copy_from_slice(data);
    *count_slot = u8::try_from(count).map_err(|_| cc::CANT_RETURN_BYTES)?;
    Ok(1 + count)
}

/// write_fru_data -- Write FRU Data (IPMI 2.0 34.3).
/// Request: FRU Device ID, offset LS byte, offset MS byte, then the bytes to write (at least one;
/// C7h otherwise). C9h for an ID above 0 or a range beyond the area; 80h while NVMRO is asserted
/// (VITA Rules 8-3, 8-5), with nothing written.
/// Response data: count written.
/// An accepted write reruns the self test (`selftest`) on the new image, so corrupt FRU data is
/// recorded in the self test results (VITA Rec 8-2): a Chassis Manager that writes an area in
/// several chunks sees 57h from Get Self Test Results between them, and 55h 00h again once the
/// image verifies. The result gates only M0 to M1; a later failure leaves M1 and M4 alone.
/// The write marks the store dirty; the firmware saves it to the flash log once no further write
/// has come for IPMC_FRU_PERSIST_DELAY_MS and NVMRO is deasserted (ipmc-fru `persist`).
pub fn write_fru_data(ctx: &mut Context, req: &Frame, out: &mut [u8]) -> Result<usize, u8> {
    let [fru_id, lo, hi, data @ ..] = req.data else {
        return Err(cc::REQ_LEN_INVALID);
    };
    if data.is_empty() {
        return Err(cc::REQ_LEN_INVALID);
    }
    check_fru_device_id(*fru_id)?;
    let offset = offset_of(*lo, *hi);
    if !ctx.fru.contains(offset, data.len()) {
        return Err(cc::PARAM_OUT_OF_RANGE);
    }
    if ctx.addr.nvmro_asserted {
        return Err(cc::WRITE_PROTECTED);
    }
    ctx.fru
        .write(offset, data)
        .map_err(|_| cc::PARAM_OUT_OF_RANGE)?;
    ctx.self_test = selftest::run(ctx.fru.bytes(), &ctx.sdr);
    let written = u8::try_from(data.len()).map_err(|_| cc::REQ_LEN_EXCEEDED)?;
    copy_out(out, &[written])
}

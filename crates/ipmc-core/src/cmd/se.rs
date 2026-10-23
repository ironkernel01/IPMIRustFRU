//! cmd/se.rs - Sensor/Event NetFn (04h) handlers: the Device SDR commands and Get Sensor Reading.
//!
//! Layouts are IPMI v2.0 rev 1.1 section 35: Table 35-2 Get Device SDR Info, Table 35-3 Get Device
//! SDR, Table 35-4 Reserve Device SDR Repository, Table 35-15 Get Sensor Reading; reservation
//! rules from sections 33.11 and 33.12. The sensors are the seven mandatory sensors of VITA 46.11
//! Table 10.2-1, all on LUN 0 of FRU 0.

use crate::cmd::app::copy_out;
use crate::cmd::require_no_data;
use crate::dispatch::{cc, Context, SensorReadError};
use crate::ipmb::Frame;

/// OP_SDR_COUNT -- Get Device SDR Info operation bit 0 = 1b: the total number of SDRs; 0b: the
/// number of sensors on the LUN the command was addressed to (Table 35-2). Bits 7:1 are reserved.
const OP_SDR_COUNT: u8 = 1;
/// SDR_INFO_LUN0_HAS_SENSORS -- Get Device SDR Info flags bit 0 (Table 35-2): LUN 0 has sensors.
/// Bit 7 = 0: static sensor population, so the 4-byte population change indicator is absent. The
/// sensor set never changes at run time and this IPMC has no firmware upgrade (VITA Rule
/// 6.2-13 sets bit 7 only for IPMCs that support firmware upgrades).
const SDR_INFO_LUN0_HAS_SENSORS: u8 = 1 << 0;
/// WHOLE_RECORD -- Get Device SDR "bytes to read" FFh: read the entire record (Table 35-3).
const WHOLE_RECORD: u8 = 0xFF;
/// NEXT_ID_LEN -- the next record ID of a Get Device SDR response (Table 35-3 response bytes 2:3).
const NEXT_ID_LEN: usize = 2;

/// get_device_sdr_info -- Get Device SDR Info (IPMI 2.0 35.2).
/// Request: none, or one operation byte (bit 0: 0b sensor count, 1b SDR count; CCh when a
/// reserved bit is set); C7h for a longer request.
/// Response data: count (sensors on LUN 0, or all SDRs), flags (LUN 0 has sensors, static
/// population).
pub fn get_device_sdr_info(ctx: &mut Context, req: &Frame, out: &mut [u8]) -> Result<usize, u8> {
    let operation = match req.data {
        [] => 0,
        [operation] => *operation,
        _ => return Err(cc::REQ_LEN_INVALID),
    };
    if operation & !OP_SDR_COUNT != 0 {
        return Err(cc::INVALID_DATA_FIELD);
    }
    let count = if operation == OP_SDR_COUNT {
        ctx.sdr.record_count()
    } else {
        ctx.sdr.sensor_count()
    };
    let count = u8::try_from(count).map_err(|_| cc::UNSPECIFIED)?;
    copy_out(out, &[count, SDR_INFO_LUN0_HAS_SENSORS])
}

/// reserve_device_sdr_repository -- Reserve Device SDR Repository (IPMI 2.0 35.4).
/// Request: none (C7h otherwise).
/// Response data: reservation ID LS byte, MS byte. Each call returns a new non-zero ID and makes
/// the previous one stale (section 33.11).
pub fn reserve_device_sdr_repository(
    ctx: &mut Context,
    req: &Frame,
    out: &mut [u8],
) -> Result<usize, u8> {
    require_no_data(req)?;
    copy_out(out, &ctx.sdr.reserve().to_le_bytes())
}

/// get_device_sdr -- Get Device SDR (IPMI 2.0 35.3).
/// Request: reservation ID (LS, MS), record ID (LS, MS), offset into the record, bytes to read
/// (FFh = the rest of the record). C7h for a length other than 6.
/// 1. A non-zero offset needs the current reservation ID: C5h otherwise (section 33.12).
/// 2. Record ID 0000h is the first record, FFFFh the last; no such record: CBh.
/// 3. An offset past the record, or an offset plus count past its end: C9h (rejected, not clamped).
/// 4. CAh when the next-ID bytes plus the requested bytes do not fit one response frame.
///
/// Response data: next record ID (LS, MS; FFFFh after the last record), the bytes.
pub fn get_device_sdr(ctx: &mut Context, req: &Frame, out: &mut [u8]) -> Result<usize, u8> {
    let [res_lo, res_hi, id_lo, id_hi, offset, count] = req.data else {
        return Err(cc::REQ_LEN_INVALID);
    };
    if *offset != 0
        && !ctx
            .sdr
            .reservation_valid(u16::from_le_bytes([*res_lo, *res_hi]))
    {
        return Err(cc::RESERVATION_INVALID);
    }
    let id = ctx
        .sdr
        .resolve(u16::from_le_bytes([*id_lo, *id_hi]))
        .ok_or(cc::NOT_PRESENT)?;
    let record = ctx.sdr.record(id).ok_or(cc::NOT_PRESENT)?;
    let offset = usize::from(*offset);
    let remaining = record
        .len()
        .checked_sub(offset)
        .ok_or(cc::PARAM_OUT_OF_RANGE)?;
    let count = if *count == WHOLE_RECORD {
        remaining
    } else {
        usize::from(*count)
    };
    if count > remaining {
        return Err(cc::PARAM_OUT_OF_RANGE);
    }
    let chunk = record
        .get(offset..offset + count)
        .ok_or(cc::PARAM_OUT_OF_RANGE)?;
    let (next, body) = out
        .split_at_mut_checked(NEXT_ID_LEN)
        .ok_or(cc::CANT_RETURN_BYTES)?;
    body.get_mut(..chunk.len())
        .ok_or(cc::CANT_RETURN_BYTES)?
        .copy_from_slice(chunk);
    next.copy_from_slice(&ctx.sdr.next_id(id).to_le_bytes());
    Ok(NEXT_ID_LEN + chunk.len())
}

/// get_sensor_reading -- Get Sensor Reading (IPMI 2.0 35.14) for the sensors of VITA Table 10.2-1.
/// Request: sensor number (C7h for another length; CBh for a number other than 0..=6).
/// Response data: reading, status, state byte, from `Context::read_sensor` (the one function
/// that maps a sensor number to its encoder; it reads the Link's view for sensor 1).
/// CBh when `read_sensor` finds no such sensor, FFh when the response buffer is too small.
pub fn get_sensor_reading(ctx: &mut Context, req: &Frame, out: &mut [u8]) -> Result<usize, u8> {
    let [number] = req.data else {
        return Err(cc::REQ_LEN_INVALID);
    };
    ctx.read_sensor(*number, &ctx.link.buses, out)
        .map_err(|error| match error {
            SensorReadError::NotPresent => cc::NOT_PRESENT,
            SensorReadError::OutputTooSmall => cc::UNSPECIFIED,
        })
}

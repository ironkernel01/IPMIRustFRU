//! cmd/app.rs - Application NetFn (06h) handlers. Each writes only its response data bytes.

use crate::cmd::require_no_data;
use crate::dispatch::{cc, Context};
use crate::identity::DEVICE_IDENTITY;
use crate::ipmb::Frame;

/// CMD_GET_DEVICE_ID -- Get Device ID command code (IPMI v2.0 Table 20-2); the link layer also
/// accepts it in the broadcast form.
pub const CMD_GET_DEVICE_ID: u8 = 0x01;
/// GET_DEVICE_ID_LEN -- Get Device ID response data length (IPMI v2.0 Table 20-2, bytes 2..16).
pub const GET_DEVICE_ID_LEN: usize = 15;

/// copy_out -- shared with the Tier-2/3 extension: copies `data` to the front of `out`; Err(UNSPECIFIED) when `out` is too small.
pub fn copy_out(out: &mut [u8], data: &[u8]) -> Result<usize, u8> {
    out.get_mut(..data.len())
        .ok_or(cc::UNSPECIFIED)?
        .copy_from_slice(data);
    Ok(data.len())
}

/// get_device_id -- Get Device ID (IPMI 2.0 20.1). Request: no data (C7h otherwise).
/// Response data, all from DEVICE_IDENTITY: device ID, device revision, firmware revision 1,
/// firmware revision 2, IPMI version, additional device support, manufacturer ID x3
/// (LSB first), product ID x2 (LSB first), auxiliary firmware revision x4.
pub fn get_device_id(_ctx: &mut Context, req: &Frame, out: &mut [u8]) -> Result<usize, u8> {
    require_no_data(req)?;
    let id = &DEVICE_IDENTITY;
    let [pid_lo, pid_hi] = id.product_id.to_le_bytes();
    let [mfr0, mfr1, mfr2] = id.manufacturer_id;
    let [aux0, aux1, aux2, aux3] = id.aux_firmware;
    let data: [u8; GET_DEVICE_ID_LEN] = [
        id.device_id,
        id.device_revision,
        id.firmware_major,
        id.firmware_minor_bcd,
        id.ipmi_version,
        id.additional_support,
        mfr0,
        mfr1,
        mfr2,
        pid_lo,
        pid_hi,
        aux0,
        aux1,
        aux2,
        aux3,
    ];
    copy_out(out, &data)
}

/// get_self_test_results -- Get Self Test Results (IPMI 2.0 20.4). Request: no data.
/// Response data: the result of the latest self test, `Context::self_test` (`selftest`): 55h 00h
/// when it passed, 57h and the failure bit field otherwise. The FRU initial checks read the same
/// value, so the state machine and the command cannot disagree.
pub fn get_self_test_results(ctx: &mut Context, req: &Frame, out: &mut [u8]) -> Result<usize, u8> {
    require_no_data(req)?;
    copy_out(out, &ctx.self_test.bytes())
}

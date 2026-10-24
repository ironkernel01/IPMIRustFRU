//! cmd - command handlers, one module per NetFn group. commands.rs maps (netFn, cmd) to them.

use crate::dispatch::cc;
use crate::ipmb::Frame;

/// app -- Application NetFn (06h) command handlers.
pub mod app;
/// se -- Sensor/Event NetFn (04h) command handlers.
pub mod se;
/// storage -- Storage NetFn (0Ah) command handlers.
pub mod storage;
/// vita -- Group Extension NetFn (2Ch) VITA 46.11 command handlers.
pub mod vita;

/// MAX_FRU_DEVICE_ID -- the largest FRU Device ID of the Managed FRUs: this IPMC represents only
/// its own FRU, FRU Device ID 0 (VITA 46.11 Rule 4.1.4.5.1-2; Get VSO Capabilities byte 7).
pub const MAX_FRU_DEVICE_ID: u8 = 0;
/// IPMC_FRU_DEVICE_ID -- the FRU Device ID of the FRU containing the IPMC (Rule 4.1.4.5.1-2;
/// Get VSO Capabilities byte 8).
pub const IPMC_FRU_DEVICE_ID: u8 = 0;

/// check_fru_device_id -- shared with the Tier-2/3 extension: the one FRU Device ID check: an ID above the maximum of the Managed
/// FRUs is "parameter out of range" C9h (VITA 46.11 Rule 4.1.1-17; Rule 4.2-3 for a Subsidiary
/// FRU, which a Tier-1 IPMC does not manage). FFh is reserved and lies above the maximum too.
pub fn check_fru_device_id(id: u8) -> Result<(), u8> {
    if id > MAX_FRU_DEVICE_ID {
        return Err(cc::PARAM_OUT_OF_RANGE);
    }
    Ok(())
}

/// require_no_data -- shared with the Tier-2/3 extension: the one check of a command that takes no request data: any data byte is
/// "request data length invalid" C7h (IPMI v2.0 Table 5-2).
pub fn require_no_data(req: &Frame) -> Result<(), u8> {
    if req.data.is_empty() {
        Ok(())
    } else {
        Err(cc::REQ_LEN_INVALID)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_fru_zero_is_accepted() {
        assert_eq!(check_fru_device_id(0), Ok(()));
        for id in [1u8, 2, 254, 255] {
            assert_eq!(check_fru_device_id(id), Err(cc::PARAM_OUT_OF_RANGE));
        }
    }
}

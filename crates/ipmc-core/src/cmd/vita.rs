//! cmd/vita.rs - Group Extension NetFn (2Ch) handlers: the VITA 46.11 commands of Table 10.1.2-1
//! that a Tier-1 IPMC implements. `dispatch` strips the VSO identifier byte from the request and
//! writes it first in the response, so the request data and the response data below start after
//! it: "byte 3" of a VITA table is response data byte 0 here.
//!
//! Layouts: Table 10.1.3.1-1 Get VSO Capabilities, 10.1.3.3-1 Get FRU Address Info, 10.1.3.7-1 Set
//! IPMB State, 10.1.3.11-1 Get Device Locator Record ID (VITA 46.11-2022 printed pages 183, 186,
//! 189, 194).

use crate::cmd::app::copy_out;
use crate::cmd::{check_fru_device_id, require_no_data, IPMC_FRU_DEVICE_ID, MAX_FRU_DEVICE_ID};
use crate::dispatch::{cc, Context};
use crate::ipmb::Frame;
use crate::link::{plan_ipmb_state, BusMode, PendingIpmbState};

// ---- Get VSO Capabilities, Table 10.1.3.1-1 ----

/// IPMC identifier byte bits 1:0 (Table 10.1.3.1-1): the Tier, 00b Tier-1, 01b Tier-2, 10b Tier-3.
const IPMC_IDENTIFIER_TIER_MASK: u32 = 0b11;
/// IPMC identifier byte: bits 5:4 Layer 00b = Layer 1 IPMC, bits 1:0 the Tier, IPMC_TIER - 1
/// (ipmc-defs guards IPMC_TIER to 1..=3, so the mask only documents the field width). The
/// Tier is 1 unless the Tier-2/3 extension is built in and raises it.
const IPMC_IDENTIFIER: u8 = ((ipmc_defs::IPMC_TIER - 1) & IPMC_IDENTIFIER_TIER_MASK) as u8;
/// IPMB capabilities byte bits 5:4: 00b = 100 kHz (STANDARD-MODE) maximum operating frequency;
/// the IPMB runs at IPMB_I2C_HZ = 100 kHz.
const IPMB_MAX_FREQUENCY_100KHZ: u8 = 0;
/// IPMB capabilities byte bits 1:0: number of supported IPMB interfaces, 00b = 1, 01b = 2.
const IPMB_INTERFACES_ONE: u8 = 0b00;
const IPMB_INTERFACES_TWO: u8 = 0b01;
/// VSO Standard byte bits 1:0 = 00b: VITA 46.11.
const VSO_STANDARD_VITA_46_11: u8 = 0b00;
/// VSO_APPROVAL_YEAR -- the approval year of the standard this IPMC follows, ANSI/VITA 46.11-2022.
const VSO_APPROVAL_YEAR: u32 = 2022;

/// vso_revision -- the VSO Standard Revision byte for an approval year: the units digit in bits
/// 7:4 and the tens digit in bits 3:0, so 2021 is 12h (Table 10.1.3.1-1 byte 6) and 2022 is 22h.
const fn vso_revision(year: u32) -> u8 {
    let units = (year % 10) as u8;
    let tens = ((year / 10) % 10) as u8;
    (units << 4) | tens
}

/// get_vso_capabilities -- Get VSO Capabilities (VITA 46.11 10.1.3.1). Request: no data beyond
/// the VSO byte (C7h otherwise).
/// Response data: IPMC identifier (Layer 1, Tier-1), IPMB capabilities (100 kHz; one interface,
/// or two when IPMB-B is fitted: Rules 4.1.1-2 and 4.1.1-3), VSO standard (VITA 46.11), VSO
/// revision (2022: 22h), max FRU Device ID (0), FRU Device ID of the IPMC (0).
pub fn get_vso_capabilities(ctx: &mut Context, req: &Frame, out: &mut [u8]) -> Result<usize, u8> {
    require_no_data(req)?;
    let interfaces = if ctx.link.config.b_fitted {
        IPMB_INTERFACES_TWO
    } else {
        IPMB_INTERFACES_ONE
    };
    copy_out(
        out,
        &[
            IPMC_IDENTIFIER,
            IPMB_MAX_FREQUENCY_100KHZ | interfaces,
            VSO_STANDARD_VITA_46_11,
            vso_revision(VSO_APPROVAL_YEAR),
            MAX_FRU_DEVICE_ID,
            IPMC_FRU_DEVICE_ID,
        ],
    )
}

// ---- Set IPMB State, Table 10.1.3.7-1 ----

/// NO_CHANGE -- IPMB-A / IPMB-B state byte FFh: the command does not change that bus.
const NO_CHANGE: u8 = 0xFF;
/// STATE_LOCAL_CONTROL_BIT -- state byte bit 0: 1b Local Control, 0b Override (isolate).
const STATE_LOCAL_CONTROL_BIT: u8 = 1 << 0;
/// LINK_ID_ALL -- state byte bits 7:1, System IPMB Link Identification: 00h selects all links
/// (also the value for a Bused System IPMB). This IPMC is on one bused System IPMB and supports
/// no other link number (01h..5Fh) and none of the reserved values (60h..7Eh): Rule 4.1.1-14.
const LINK_ID_ALL: u8 = 0;
/// SPEED_ALL_100KHZ -- the optional IPMB Speed byte with both bus fields 00b (100 kHz) and the
/// reserved bits 7:4 zero. The IPMB runs at 100 kHz only; 01b (400 kHz FAST-MODE) and the
/// reserved values 10b and 11b are unsupported, and so is a non-zero reserved nibble: Rule 4.1.1-14.
const SPEED_ALL_100KHZ: u8 = 0;

/// parse_bus_state -- one state byte: None for FFh (no change), else the mode of bit 0.
/// Err(CCh) for a link identification other than 00h.
fn parse_bus_state(byte: u8) -> Result<Option<BusMode>, u8> {
    if byte == NO_CHANGE {
        return Ok(None);
    }
    if byte >> 1 != LINK_ID_ALL {
        return Err(cc::INVALID_DATA_FIELD);
    }
    Ok(Some(if byte & STATE_LOCAL_CONTROL_BIT != 0 {
        BusMode::LocalControl
    } else {
        BusMode::Isolated
    }))
}

/// set_ipmb_state -- Set IPMB State (VITA 46.11 10.1.3.7).
/// Request: IPMB-A state, IPMB-B state, optional IPMB speed. C7h for another length.
/// 1. A parameter this IPMC does not support (link identification other than 00h, a speed other
///    than 100 kHz) -> CCh with no change (Rule 4.1.1-14).
/// 2. `link::plan_ipmb_state` applies the Link's one rule to the Link's view: Local Control on a
///    bus that is not fitted -> CCh (Rule 4.1.1-14), a change that would isolate this IPMC from
///    both buses -> CCh (Rule 4.1.8-3); no change in either case.
/// 3. Otherwise the change goes to `Context::pending_ipmb`; `Link::poll` applies it right after
///    this handler returns, before the response is queued.
///
/// Response data: none.
pub fn set_ipmb_state(ctx: &mut Context, req: &Frame, _out: &mut [u8]) -> Result<usize, u8> {
    let (a, b, speed) = match req.data {
        [a, b] => (*a, *b, SPEED_ALL_100KHZ),
        [a, b, speed] => (*a, *b, *speed),
        _ => return Err(cc::REQ_LEN_INVALID),
    };
    let (a, b) = (parse_bus_state(a)?, parse_bus_state(b)?);
    if speed != SPEED_ALL_100KHZ {
        return Err(cc::INVALID_DATA_FIELD);
    }
    plan_ipmb_state(&ctx.link.buses, a, b).map_err(|_| cc::INVALID_DATA_FIELD)?;
    ctx.pending_ipmb = Some(PendingIpmbState { a, b });
    Ok(0)
}

// ---- Get Device Locator Record ID, Table 10.1.3.11-1 ----

/// get_device_locator_record_id -- Get Device Locator Record ID (VITA 46.11 10.1.3.11).
/// Request: FRU Device ID (C7h for another length; C9h above 0). FRU Device ID 0 returns the
/// Management Controller Device Locator Record ID.
/// Response data: record ID LS byte, MS byte.
pub fn get_device_locator_record_id(
    ctx: &mut Context,
    req: &Frame,
    out: &mut [u8],
) -> Result<usize, u8> {
    let [fru_id] = req.data else {
        return Err(cc::REQ_LEN_INVALID);
    };
    check_fru_device_id(*fru_id)?;
    copy_out(out, &ctx.sdr.locator_record_id().to_le_bytes())
}

// ---- Get FRU Address Info, Table 10.1.3.3-1 ----

/// ADDRESS_KEY_PHYSICAL -- Address Key Type 03h = Physical Address, the only value defined for
/// an IPM Controller (Table 10.1.3.3-1).
const ADDRESS_KEY_PHYSICAL: u8 = 0x03;
/// SITE_TYPE_FRONT_LOADING_VPX_MODULE -- Site Type 00h, Front Loading VPX Plug-In Module (Table
/// 4.1.4.3-1, printed page 43).
const SITE_TYPE_FRONT_LOADING_VPX_MODULE: u8 = 0x00;
/// RESERVED_FF -- response byte 5, Reserved, value FFh (Table 10.1.3.3-1).
const RESERVED_FF: u8 = 0xFF;

/// get_fru_address_info -- Get FRU Address Info (VITA 46.11 10.1.3.3).
/// Request: none (the FRU containing the IPMC), or the FRU Device ID alone, or FRU Device ID,
/// address key type, address key (the Site Number) and site type. C7h for another length.
/// An ID above 0 -> C9h (the ID is ignored when a physical address is given); a key type other
/// than 03h -> CCh; a site number or site type that is not this FRU's -> D3h (Rule 4.1.1-18:
/// no such FRU is present).
/// Response data: hardware address, IPMB address, reserved FFh, FRU Device ID 0, site number,
/// site type. The Site Number is GA[4:0], the hardware address minus 40h (Rule 4.1.4.3-3) and
/// the Site Type is 00h, Front Loading VPX Plug-In Module. The optional bytes 9 (reserved) and
/// 10 (address on IPMI channel 7) are not returned: there is no Subsidiary FRU.
pub fn get_fru_address_info(ctx: &mut Context, req: &Frame, out: &mut [u8]) -> Result<usize, u8> {
    match req.data {
        [] => {}
        [fru_id] => check_fru_device_id(*fru_id)?,
        [_fru_id, key_type, site_number, site_type] => {
            if *key_type != ADDRESS_KEY_PHYSICAL {
                return Err(cc::INVALID_DATA_FIELD);
            }
            if *site_number != ctx.addr.ga || *site_type != SITE_TYPE_FRONT_LOADING_VPX_MODULE {
                return Err(cc::DESTINATION_UNAVAILABLE);
            }
        }
        _ => return Err(cc::REQ_LEN_INVALID),
    }
    copy_out(
        out,
        &[
            ctx.addr.hw_address,
            ctx.addr.ipmb_address,
            RESERVED_FF,
            IPMC_FRU_DEVICE_ID,
            ctx.addr.ga,
            SITE_TYPE_FRONT_LOADING_VPX_MODULE,
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vso_revision_nibbles_follow_the_spec_example() {
        assert_eq!(vso_revision(2021), 0x12);
        assert_eq!(vso_revision(2022), 0x22);
        assert_eq!(vso_revision(2015), 0x51);
        assert_eq!(vso_revision(2099), 0x99);
    }

    #[test]
    fn bus_state_bytes_parse_per_table_10_1_3_7_1() {
        assert_eq!(parse_bus_state(0xFF), Ok(None));
        assert_eq!(parse_bus_state(0x00), Ok(Some(BusMode::Isolated)));
        assert_eq!(parse_bus_state(0x01), Ok(Some(BusMode::LocalControl)));
        // Link numbers 1..=95 and the reserved 60h..7Eh are unsupported (bits 7:1 non-zero).
        for byte in [0x02, 0x03, 0x0B, 0xBF, 0xC0, 0xFE] {
            assert_eq!(
                parse_bus_state(byte),
                Err(cc::INVALID_DATA_FIELD),
                "{byte:02X}"
            );
        }
    }
}

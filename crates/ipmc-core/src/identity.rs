//! identity.rs - the device identity the IPMC reports, assembled from `ipmc_defs` values.
//!
//! Get Device ID (cmd/app.rs) reads every value from `DEVICE_IDENTITY`. The numbers live in
//! ipmi_defs.h (IPMC_DEVICE_ID, IPMC_PRODUCT_ID, ...); this file holds no tunable literal, only
//! the wire layout (which bit or byte each value lands in) and the firmware revision derived
//! from the crate version. The FRU inventory strings (IPMC_FRU_*) join this table when the FRU
//! image is assembled, so both commands report the same identity from one place.

// Additional Device Support bit positions (IPMI v2.0 Table 20-2, response byte 7).
const SUPPORT_SENSOR_BIT: u32 = 0;
const SUPPORT_SDR_REPOSITORY_BIT: u32 = 1;
const SUPPORT_SEL_BIT: u32 = 2;
const SUPPORT_FRU_INVENTORY_BIT: u32 = 3;
const SUPPORT_EVENT_RECEIVER_BIT: u32 = 4;
const SUPPORT_EVENT_GENERATOR_BIT: u32 = 5;
const SUPPORT_BRIDGE_BIT: u32 = 6;
const SUPPORT_CHASSIS_BIT: u32 = 7;
/// Device Revision byte: bit 7 is "provides Device SDRs"; bits 6:4 are reserved (0); bits 3:0
/// hold the device revision (IPMI v2.0 Table 20-2, byte 3).
const REVISION_SDRS_BIT: u32 = 7;
/// Mask of one byte; the bytes of a multi-byte field are taken with it.
const BYTE_MASK: u32 = 0xFF;
const BITS_PER_BYTE: u32 = 8;

/// byte_of -- byte `index` (0 = least significant) of `value`. The cast keeps the low 8 bits
/// that the mask has already isolated.
const fn byte_of(value: u32, index: u32) -> u8 {
    ((value >> (BITS_PER_BYTE * index)) & BYTE_MASK) as u8
}

/// Additional Device Support byte (response byte 7), one bit per ipmi_defs.h IPMC_SUPPORT_*.
const ADDITIONAL_SUPPORT: u8 = byte_of(
    (ipmc_defs::IPMC_SUPPORT_SENSOR << SUPPORT_SENSOR_BIT)
        | (ipmc_defs::IPMC_SUPPORT_SDR_REPOSITORY << SUPPORT_SDR_REPOSITORY_BIT)
        | (ipmc_defs::IPMC_SUPPORT_SEL << SUPPORT_SEL_BIT)
        | (ipmc_defs::IPMC_SUPPORT_FRU_INVENTORY << SUPPORT_FRU_INVENTORY_BIT)
        | (ipmc_defs::IPMC_SUPPORT_EVENT_RECEIVER << SUPPORT_EVENT_RECEIVER_BIT)
        | (ipmc_defs::IPMC_SUPPORT_EVENT_GENERATOR << SUPPORT_EVENT_GENERATOR_BIT)
        | (ipmc_defs::IPMC_SUPPORT_BRIDGE << SUPPORT_BRIDGE_BIT)
        | (ipmc_defs::IPMC_SUPPORT_CHASSIS << SUPPORT_CHASSIS_BIT),
    0,
);

/// Device Revision byte (response byte 3).
const DEVICE_REVISION: u8 = byte_of(
    (ipmc_defs::IPMC_DEVICE_PROVIDES_SDRS << REVISION_SDRS_BIT) | ipmc_defs::IPMC_DEVICE_REVISION,
    0,
);

/// Product ID, sent least significant byte first (IPMI v2.0 Table 20-2, bytes 11:12). The
/// define is guarded in ipmc-defs to 16 bits and not FFFFh (reserved).
const PRODUCT_ID: u16 = (ipmc_defs::IPMC_PRODUCT_ID & u16::MAX as u32) as u16;

/// Firmware revision fields from the crate version, 0.1.x (owner decision 1).
const FIRMWARE_MAJOR: u8 = version_field(env!("CARGO_PKG_VERSION"), 0);
const FIRMWARE_MINOR: u8 = version_field(env!("CARGO_PKG_VERSION"), 1);
const _: () = assert!(
    FIRMWARE_MAJOR == 0 && FIRMWARE_MINOR == 1,
    "the crate version must be 0.1.x; update the Get Device ID decision before changing it"
);
// The FRU product version string is the same major.minor as the crate version, so the two
// cannot drift: Get Device ID and the FRU product area report one firmware release.
const _: () = assert!(
    version_matches_major_minor(
        ipmc_defs::IPMC_FRU_PRODUCT_VERSION,
        env!("CARGO_PKG_VERSION")
    ),
    "IPMC_FRU_PRODUCT_VERSION in ipmi_defs.h must equal the crate's major.minor version"
);
/// Firmware minor revision as two BCD digits (IPMI v2.0 Table 20-2, byte 5).
const FIRMWARE_MINOR_BCD: u8 = ((FIRMWARE_MINOR / 10) << 4) | (FIRMWARE_MINOR % 10);

/// version_field -- field `index` (0 = major, 1 = minor) of a "major.minor.patch" string, at
/// compile time. A malformed string or a value above 99 gives 0xFF, which the assertion
/// above rejects.
const fn version_field(version: &str, index: usize) -> u8 {
    const BAD: u8 = 0xFF;
    const MAX: u32 = 99;
    let bytes = version.as_bytes();
    let mut field = 0;
    let mut value: u32 = 0;
    let mut digits = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'.' {
            if field == index {
                break;
            }
            field += 1;
        } else if b.is_ascii_digit() {
            if field == index {
                value = value * 10 + (b - b'0') as u32;
                digits += 1;
                if value > MAX {
                    return BAD;
                }
            }
        } else {
            break;
        }
        i += 1;
    }
    if digits == 0 || field < index {
        return BAD;
    }
    value as u8
}

/// version_matches_major_minor -- true when `text` is exactly the "major.minor" prefix of the
/// "major.minor.patch" string `version` (the text before its second dot, or all of it).
const fn version_matches_major_minor(text: &str, version: &str) -> bool {
    let (text, version) = (text.as_bytes(), version.as_bytes());
    let mut dots = 0;
    let mut end = 0;
    while end < version.len() {
        if version[end] == b'.' {
            dots += 1;
            if dots == 2 {
                break;
            }
        }
        end += 1;
    }
    if text.len() != end {
        return false;
    }
    let mut i = 0;
    while i < end {
        if text[i] != version[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// DeviceIdentity -- the values Get Device ID reports (IPMI v2.0 Table 20-2).
pub struct DeviceIdentity {
    /// Device ID (byte 2).
    pub device_id: u8,
    /// Device Revision (byte 3), including the "provides Device SDRs" bit.
    pub device_revision: u8,
    /// Firmware revision, major in binary (byte 4 bits 6:0; bit 7 = 0, normal operation).
    pub firmware_major: u8,
    /// Firmware revision, minor as BCD (byte 5).
    pub firmware_minor_bcd: u8,
    /// IPMI version (byte 6).
    pub ipmi_version: u8,
    /// Additional Device Support (byte 7).
    pub additional_support: u8,
    /// Manufacturer ID, least significant byte first (bytes 8:10). 000000h until an IANA
    /// Private Enterprise Number is registered; another party's number is never borrowed.
    pub manufacturer_id: [u8; 3],
    /// Product ID (bytes 11:12), sent little-endian.
    pub product_id: u16,
    /// Auxiliary Firmware Revision (bytes 13:16).
    pub aux_firmware: [u8; 4],
}

/// DEVICE_IDENTITY -- the identity of this IPMC (PLAN.md section 3.7, decision 1).
pub const DEVICE_IDENTITY: DeviceIdentity = DeviceIdentity {
    device_id: byte_of(ipmc_defs::IPMC_DEVICE_ID, 0),
    device_revision: DEVICE_REVISION,
    firmware_major: FIRMWARE_MAJOR,
    firmware_minor_bcd: FIRMWARE_MINOR_BCD,
    ipmi_version: byte_of(ipmc_defs::IPMC_IPMI_VERSION, 0),
    additional_support: ADDITIONAL_SUPPORT,
    manufacturer_id: [
        byte_of(ipmc_defs::IPMC_MANUFACTURER_ID, 0),
        byte_of(ipmc_defs::IPMC_MANUFACTURER_ID, 1),
        byte_of(ipmc_defs::IPMC_MANUFACTURER_ID, 2),
    ],
    product_id: PRODUCT_ID,
    aux_firmware: ipmc_defs::IPMC_AUX_FIRMWARE.to_le_bytes(),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_field_parses_and_rejects() {
        assert_eq!(version_field("0.1.0", 0), 0);
        assert_eq!(version_field("0.1.0", 1), 1);
        assert_eq!(version_field("12.34.56", 1), 34);
        assert_eq!(version_field("1.2.3-rc1", 1), 2);
        assert_eq!(version_field("1.2", 2), 0xFF);
        assert_eq!(version_field("1..2", 1), 0xFF);
        assert_eq!(version_field("100.1.0", 0), 0xFF);
        assert_eq!(version_field("x.1.0", 0), 0xFF);
    }

    #[test]
    fn product_version_guard_compares_major_minor() {
        assert!(version_matches_major_minor("0.1", "0.1.0"));
        assert!(version_matches_major_minor("0.1", "0.1.9-rc1"));
        assert!(version_matches_major_minor("12.34", "12.34.5"));
        assert!(version_matches_major_minor("0.1", "0.1"));
        assert!(!version_matches_major_minor("0.2", "0.1.0"));
        assert!(!version_matches_major_minor("0.1.0", "0.1.0"));
        assert!(!version_matches_major_minor("0", "0.1.0"));
        assert!(!version_matches_major_minor("", "0.1.0"));
        assert!(!version_matches_major_minor("0.1", "0.10.0"));
    }

    #[test]
    fn identity_matches_decision_one() {
        assert_eq!(DEVICE_IDENTITY.device_id, 0x01);
        assert_eq!(DEVICE_IDENTITY.device_revision, 0x80);
        assert_eq!(DEVICE_IDENTITY.firmware_minor_bcd, 0x01);
        assert_eq!(DEVICE_IDENTITY.additional_support, 0x09);
        assert_eq!(DEVICE_IDENTITY.product_id.to_le_bytes(), [0x4B, 0x49]);
    }
}

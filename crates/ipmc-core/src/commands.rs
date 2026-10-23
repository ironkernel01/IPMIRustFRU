//! commands.rs - the command table: one row per command, one line per row.
//!
//! Adding a command is adding one row. dispatch.rs is the only caller of `find`.

use crate::cmd::{app, se, storage, vita};
use crate::dispatch::Handler;
use crate::ipmb::{NETFN_APP, NETFN_GROUP_EXT, NETFN_SE, NETFN_STORAGE};

/// Command -- one table row: the request (netFn, cmd), its log name, the spec section that
/// defines it and the handler.
pub struct Command {
    /// Request NetFn (even).
    pub netfn: u8,
    /// Command code.
    pub cmd: u8,
    /// Name printed in trace lines.
    pub name: &'static str,
    /// Spec section that defines the command.
    pub spec: &'static str,
    /// Handler run for a matching request.
    pub handler: Handler,
}

/// cmd -- builds a table row.
pub(crate) const fn cmd(
    netfn: u8,
    cmd: u8,
    name: &'static str,
    spec: &'static str,
    handler: Handler,
) -> Command {
    Command {
        netfn,
        cmd,
        name,
        spec,
        handler,
    }
}

/// COMMANDS -- every command this IPMC answers; any other request gets C1h.
#[rustfmt::skip]
pub const COMMANDS: &[Command] = &[
    // Application, NetFn 06h, IPMI 2.0 chapter 20
    cmd(NETFN_APP, 0x01, "Get Device ID", "IPMI 2.0 20.1", app::get_device_id),
    cmd(NETFN_APP, 0x04, "Get Self Test Results", "IPMI 2.0 20.4", app::get_self_test_results),
    // Sensor/Event, NetFn 04h, IPMI 2.0 chapter 35
    cmd(NETFN_SE, 0x20, "Get Device SDR Info", "IPMI 2.0 35.2", se::get_device_sdr_info),
    cmd(NETFN_SE, 0x21, "Get Device SDR", "IPMI 2.0 35.3", se::get_device_sdr),
    cmd(NETFN_SE, 0x22, "Reserve Device SDR Repository", "IPMI 2.0 35.4", se::reserve_device_sdr_repository),
    cmd(NETFN_SE, 0x2D, "Get Sensor Reading", "IPMI 2.0 35.14", se::get_sensor_reading),
    // Storage, NetFn 0Ah, IPMI 2.0 chapter 34
    cmd(NETFN_STORAGE, 0x10, "Get FRU Inventory Area Info", "IPMI 2.0 34.1", storage::get_fru_inventory_area_info),
    cmd(NETFN_STORAGE, 0x11, "Read FRU Data", "IPMI 2.0 34.2", storage::read_fru_data),
    cmd(NETFN_STORAGE, 0x12, "Write FRU Data", "IPMI 2.0 34.3", storage::write_fru_data),
    // Group Extension, NetFn 2Ch, VITA 46.11 section 10.1.3
    cmd(NETFN_GROUP_EXT, 0x00, "Get VSO Capabilities", "VITA 46.11 10.1.3.1", vita::get_vso_capabilities),
    cmd(NETFN_GROUP_EXT, 0x09, "Set IPMB State", "VITA 46.11 10.1.3.7", vita::set_ipmb_state),
    cmd(NETFN_GROUP_EXT, 0x0D, "Get Device Locator Record ID", "VITA 46.11 10.1.3.11", vita::get_device_locator_record_id),
    cmd(NETFN_GROUP_EXT, 0x40, "Get FRU Address Info", "VITA 46.11 10.1.3.3", vita::get_fru_address_info),
];

/// find_in -- the row of `table` for (netfn, cmd); linear scan, the table is small.
pub fn find_in(table: &[Command], netfn: u8, cmd: u8) -> Option<&Command> {
    table
        .iter()
        .find(|row| row.netfn == netfn && row.cmd == cmd)
}

/// find -- the row of COMMANDS for (netfn, cmd), None when the command is not supported.
pub fn find(netfn: u8, cmd: u8) -> Option<&'static Command> {
    find_in(COMMANDS, netfn, cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipmb::{NETFN_GROUP_EXT, NETFN_SE, NETFN_STORAGE};

    #[test]
    fn rows_are_unique_and_documented() {
        for (i, row) in COMMANDS.iter().enumerate() {
            assert!(!row.name.is_empty(), "row {i} has no name");
            assert!(!row.spec.is_empty(), "row {i} has no spec");
            for other in &COMMANDS[i + 1..] {
                assert!(
                    (row.netfn, row.cmd) != (other.netfn, other.cmd),
                    "duplicate row {:02X}h/{:02X}h",
                    row.netfn,
                    row.cmd
                );
            }
        }
    }

    #[test]
    fn find_hits_and_misses() {
        assert_eq!(find(NETFN_APP, 0x01).map(|r| r.name), Some("Get Device ID"));
        assert_eq!(
            find(NETFN_APP, 0x04).map(|r| r.name),
            Some("Get Self Test Results")
        );
        assert!(find(NETFN_APP, 0x55).is_none());
        assert!(find(NETFN_SE, 0x01).is_none());
        assert!(find(NETFN_STORAGE, 0x01).is_none());
        assert!(find(NETFN_GROUP_EXT, 0x01).is_none());
    }
}

// ipmb_cmds.rs - the IPMB STATS and IPMB STATE console commands.
//
// Read-only views of the link layer (ipmc_runtime::link::IpmbLink): the per-bus counters of the
// core link, the driver and master counters the board's `IpmbBus` reports, the fault streaks and
// the bus state. Nothing here changes a counter or runs in an interrupt handler.

use ipmc_core::link::Link;
use ipmc_core::queue::Bus;
use ipmc_core::sensor;
use ipmc_hal::board::Board;
use ipmc_hal::ipmb::{IpmbBus, MasterError};
use ipmc_menu::{Args, CmdResult};

use crate::console::{Context, LinkHome};
use crate::link::IpmbLink;
use crate::out;
use crate::text::ConsoleText;

/// FAULT_STREAK -- consecutive bus-fault errors that make a fault persistent (IPMB_FAULT_STREAK).
const FAULT_STREAK: u32 = ipmc_defs::IPMB_FAULT_STREAK;

/// print_driver_stats -- the driver, master and fault counter lines of one fitted bus.
fn print_driver_stats<B: Board>(con: &mut impl ConsoleText, link: &IpmbLink<B>, bus: Bus) {
    let d = link.ipmb().slave_stats(bus);
    let (streak, faulted) = link.fault_state(bus);
    let name = bus.name();
    out!(con, "IPMB-{} drv:   rx={} overrun={} q-full={} empty={} restart-drop={} slave-rd={} rd-end={}\n",
        name,
        d.rx_frames,
        d.rx_overrun,
        d.rx_queue_full,
        d.rx_empty,
        d.restart_drops,
        d.slave_reads,
        d.slave_read_end
    );
    out!(
        con,
        "IPMB-{} drv err: berr={} arlo={} af={} ovr={} timeout={} master-berr-spurious={}\n",
        name,
        d.bus_error,
        d.arbitration_lost,
        d.ack_failure,
        d.overrun,
        d.timeout,
        d.master_berr_spurious
    );
    out!(
        con,
        "IPMB-{} fault: streak={}/{} state={} busy-errata-resets={}\n",
        name,
        streak,
        FAULT_STREAK,
        if faulted { "FAULTED" } else { "ok" },
        d.busy_errata_resets
    );
    let m = link.ipmb().master_stats().bus(bus);
    let count = |kind: MasterError| m.errors.get(kind.index()).copied().unwrap_or(0);
    out!(con, "IPMB-{} master: ok={} busy={} arb-lost={} arb-retries={} addr-nack={} data-nack={} stretch={} scl-low={} sda-low={} scl-high={} sda-high={}\n",
        name,
        m.writes_ok,
        count(MasterError::BusBusy),
        count(MasterError::ArbitrationLost),
        m.arb_retries,
        count(MasterError::AddressNack),
        count(MasterError::DataNack),
        count(MasterError::StretchTimeout),
        count(MasterError::SclStuckLow),
        count(MasterError::SdaStuckLow),
        count(MasterError::SclStuckHigh),
        count(MasterError::SdaStuckHigh)
    );
}

/// print_bus_stats -- the three counter lines of one bus.
fn print_bus_stats(con: &mut impl ConsoleText, link: &Link, bus: Bus) {
    let c = link.counters(bus);
    let e = &c.rx_errors;
    let name = bus.name();
    out!(con, "IPMB-{} rx:    ok={} not-for-us={} response={} answers={} unavailable={} power-up-window={} ingress-full={}\n",
        name,
        c.rx_ok,
        c.not_for_us,
        c.rx_response,
        c.rx_answers,
        c.bus_unavailable,
        c.rx_power_up_window,
        c.ingress_full
    );
    out!(
        con,
        "IPMB-{} rx err: short={} long={} chk1={} chk2={} address={} other={}\n",
        name,
        e.too_short,
        e.too_long,
        e.header_checksum,
        e.data_checksum,
        e.invalid_address,
        e.other
    );
    out!(con, "IPMB-{} tx:    ok={} requests={} fail={} fail-streak={} dropped={} resends={} replays={}\n",
        name,
        c.tx_ok,
        c.tx_requests,
        c.tx_fail,
        c.tx_fail_streak,
        c.tx_dropped,
        c.resends,
        c.replays
    );
}

/// cmd_ipmb_stats -- IPMB STATS: the per-bus counters, the driver and master counters of each
/// fitted bus, and the link-wide counters.
pub fn cmd_ipmb_stats<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    _args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    let ipmb = &*ctx.ipmb;
    let link = ipmb.link();
    for bus in Bus::BOTH {
        print_bus_stats(con, link, bus);
        if link.bus_state(bus).fitted {
            print_driver_stats(con, ipmb, bus);
        }
    }
    match ipmb.ipmb().master_stats().scl_period_ns() {
        Some(ns) => out!(con, "IPMB master:   last write SCL period {} ns\n", ns),
        None => out!(con, "IPMB master:   no write yet\n"),
    }
    out!(
        con,
        "IPMB link:     cache-evictions={} late-responses={} dispatch-errors={} egress-full={}\n",
        link.evictions(),
        link.late_responses(),
        link.dispatch_errors(),
        link.egress_dropped()
    );
    Ok(())
}

/// cmd_ipmb_state -- IPMB STATE: per-bus fitted, mode, enable and fault state, the transmit
/// policy and the IPMB Link sensor reading bytes.
pub fn cmd_ipmb_state<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    _args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    let link = ctx.ipmb.link();
    for bus in Bus::BOTH {
        let s = link.bus_state(bus);
        out!(
            con,
            "IPMB-{}: {}, {}, {}, local status {}\n",
            bus.name(),
            if s.fitted { "fitted" } else { "not fitted" },
            s.mode.name(),
            if s.enabled() { "enabled" } else { "disabled" },
            s.status.name()
        );
    }
    out!(
        con,
        "TX policy: {} (from ipmc-defs IPMB_COMPLIANCE)\n",
        ipmc_core::link::tx_policy_name(link.config().tx_policy)
    );
    let mut data = [0u8; sensor::READING_LEN];
    let reading = ctx.ipmb.dispatch().read_sensor(
        sensor::SENSOR_SYSTEM_IPMB_LINK,
        &link.view().buses,
        &mut data,
    );
    if reading.is_ok() {
        out!(
            con,
            "Link sensor data (sensor 1): {:02X} {:02X} {:02X}\n",
            data[0],
            data[1],
            data[2]
        );
    }
    Ok(())
}

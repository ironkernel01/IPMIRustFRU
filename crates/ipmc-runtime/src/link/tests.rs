// link/tests.rs - the IPMB link service over the host test board: frame order across the buses,
// the bus-fault streak and its evidence-gated clear, the erase pause and its deferrals, the urgent
// save before a Cold Reset, and the Cold and Warm Reset paths.

extern crate std;

use super::*;
use crate::sim::{
    IpmbState, Reset, Sim, SimClock, SimCycles, SimIpmb, SimSerial, SimStraps, SimSystem,
};
use core::cell::RefCell;
use ipmc_core::ipmb::{Header, NETFN_APP};
use ipmc_core::link::CM_ADDR8;
use ipmc_fru::persist::{Persist, RECORDS_PER_SECTOR};
use ipmc_fru::RamFlash;
use ipmc_hal::system::ResetCause;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;

/// Rig -- a link over the test board and the handles a test reads.
struct Rig {
    link: IpmbLink<Sim>,
    bus: Rc<RefCell<IpmbState>>,
    clock: SimClock,
    cycles: SimCycles,
    con: SimSerial,
}

impl Rig {
    /// text -- the console text since the last call.
    fn text(&self) -> String {
        self.con.take_text()
    }

    /// erases -- sector erases the log flash ran.
    fn erases(&mut self) -> usize {
        self.link
            .fru_log
            .parts()
            .map_or(0, |(_, flash)| flash.erases)
    }

    /// events -- the calls the buses saw, joined.
    fn events(&self) -> String {
        self.bus.borrow().events.join(",")
    }
}

/// both_running -- a started pair of buses with both targets up (whatever IPMB_BUS_B_ENABLE says).
fn both_running(mut ipmb: SimIpmb) -> StartedBuses<SimIpmb> {
    let (rx_a, _) = ipmb.bring_up(Bus::A, 0x82).expect("A starts");
    let (rx_b, _) = ipmb.bring_up(Bus::B, 0x82).expect("B starts");
    ipmb.state.borrow_mut().events.clear();
    StartedBuses {
        ipmb,
        rx_a: Some(rx_a),
        rx_b: Some(rx_b),
        fault_a: None,
        fault_b: None,
    }
}

/// link_over -- a link on `buses` and `flash`; the lines of the FRU log start are cleared from the
/// console.
fn link_over(
    ipmb: &Rc<RefCell<IpmbState>>,
    started: impl FnOnce(&mut SimSerial) -> StartedBuses<SimIpmb>,
    flash: RamFlash,
) -> Rig {
    let mut con = SimSerial::roomy();
    let (fru_log, fru_image) = FruLog::boot(Ok(flash), false, &mut con);
    let buses = started(&mut con);
    let (clock, cycles) = (SimClock::default(), SimCycles::default());
    let mut link = IpmbLink::new(LinkArgs {
        buses,
        fru_log,
        fru_image,
        kept: (0, 0),
        clock: clock.clone(),
        cycles: cycles.clone(),
        straps: SimStraps::default(),
        con: con.clone(),
    });
    link.log_mut().capture_mut().set_enabled(true);
    con.take_text();
    Rig {
        link,
        bus: Rc::clone(ipmb),
        clock,
        cycles,
        con,
    }
}

/// rig -- both buses running, an erased log sector.
fn rig() -> Rig {
    rig_with(RamFlash::new())
}

/// rig_with -- both buses running over `flash`.
fn rig_with(flash: RamFlash) -> Rig {
    let ipmb = SimIpmb::default();
    let state = Rc::clone(&ipmb.state);
    link_over(&state, |_| both_running(ipmb), flash)
}

/// full_flash -- a log sector with every record slot written: the next save must erase.
fn full_flash() -> RamFlash {
    let mut flash = RamFlash::new();
    let mut persist = Persist::new();
    persist
        .boot(&mut flash, false)
        .expect("a blank sector boots");
    let image = FruStore::new();
    for _ in 0..RECORDS_PER_SECTOR {
        persist
            .save(&mut flash, image.image(), image.nv_settings())
            .expect("a slot is free");
    }
    assert!(persist.save_needs_erase());
    flash
}

/// get_device_id -- a Get Device ID request from the Chassis Manager to `dest`.
fn get_device_id(dest: u8, seq: u8) -> Vec<u8> {
    let header = Header {
        dest,
        netfn: NETFN_APP,
        dest_lun: 0,
        src: CM_ADDR8,
        seq,
        src_lun: 0,
        cmd: 0x01,
    };
    let mut buf = [0u8; IPMB_MAX_LEN];
    let n = ipmb::encode_request(&header, &[], &mut buf).expect("the request fits");
    buf[..n].to_vec()
}

/// received_order -- the buses of the frames the capture log holds as received, oldest first.
fn received_order(link: &IpmbLink<Sim>) -> Vec<Bus> {
    let mut order = Vec::new();
    link.log().capture().for_each(|record| {
        if let Kind::Frame {
            bus, dir: Dir::In, ..
        } = record.kind
        {
            order.push(bus);
        }
    });
    order
}

#[test]
fn a_request_is_answered_through_the_board() {
    let mut rig = rig();
    let own = rig.link.link().own_address();
    rig.bus
        .borrow_mut()
        .push_frame(Bus::A, &get_device_id(own, 3), 0);
    assert_eq!(rig.link.service(), None);
    let state = rig.bus.borrow();
    assert_eq!(state.outbox.len(), 1);
    let (bus, bytes, _) = &state.outbox[0];
    assert_eq!(*bus, Bus::A);
    let reply = ipmb::parse(bytes).expect("the response parses");
    assert!(reply.header.is_response());
    assert_eq!((reply.header.cmd, reply.header.seq), (0x01, 3));
}

#[test]
fn frames_drain_in_stop_time_order_across_the_buses() {
    // (A's STOP time, B's STOP time, the bus drained first): equal times go to A; the
    // comparison is a wrapping difference.
    let cases: [(u32, u32, Bus); 4] = [
        (10, 10, Bus::A),
        (10, 20, Bus::A),
        (20, 10, Bus::B),
        (5, u32::MAX - 5, Bus::B),
    ];
    for (a_stop, b_stop, first) in cases {
        let mut rig = rig();
        {
            let mut state = rig.bus.borrow_mut();
            state.push_frame(Bus::A, &[0x01, 0x02, 0x03], a_stop);
            state.push_frame(Bus::B, &[0x04, 0x05, 0x06], b_stop);
        }
        rig.link.service();
        let order = received_order(&rig.link);
        assert_eq!(order.len(), 2, "A {a_stop} B {b_stop}");
        assert_eq!(order[0], first, "A {a_stop} B {b_stop}");
    }
}

#[test]
fn the_fault_streak_marks_a_bus_and_the_clear_waits_for_the_lines() {
    let mut rig = rig();
    let set_errors = |rig: &Rig, count: u32| {
        rig.bus.borrow_mut().bus[Bus::A.index()].stats.bus_error = count;
    };
    rig.bus.borrow_mut().bus[Bus::A.index()].lines_high = true;
    // One short of the streak: counted, not a fault.
    set_errors(&rig, FAULT_STREAK - 1);
    rig.link.service();
    assert_eq!(rig.link.fault_state(Bus::A), (FAULT_STREAK - 1, false));
    assert_eq!(rig.text(), "");
    // The streak is reached: the bus is marked, the link records the local status.
    set_errors(&rig, FAULT_STREAK);
    rig.link.service();
    assert_eq!(rig.link.fault_state(Bus::A), (FAULT_STREAK, true));
    assert_ne!(rig.link.link().bus_state(Bus::A).status, LocalStatus::None);
    assert_eq!(rig.text(), "IPMB-A fault: slave bus error\r\n");
    // The lines read high since t = 0, but a break in the evidence restarts the wait.
    rig.clock.advance(FAULT_CLEAR_MS - 1);
    rig.bus.borrow_mut().bus[Bus::A.index()].lines_high = false;
    rig.link.service();
    rig.bus.borrow_mut().bus[Bus::A.index()].lines_high = true;
    rig.clock.advance(1);
    rig.link.service();
    rig.clock.advance(FAULT_CLEAR_MS - 1);
    rig.link.service();
    assert_eq!(rig.bus.borrow().bus[Bus::A.index()].cleared, 0);
    assert_eq!(rig.text(), "");
    // A full FAULT_CLEAR_MS of high lines: the clear runs and the streak starts over.
    rig.clock.advance(1);
    rig.link.service();
    assert_eq!(rig.bus.borrow().bus[Bus::A.index()].cleared, 1);
    assert_eq!(rig.text(), "IPMB-A fault cleared\r\n");
    assert_eq!(rig.link.fault_state(Bus::A), (0, false));
    assert_eq!(rig.link.link().bus_state(Bus::A).status, LocalStatus::None);
}

#[test]
fn a_bus_that_failed_to_start_is_reported_once_and_started_when_the_lines_are_high() {
    let ipmb = SimIpmb::default();
    let state = Rc::clone(&ipmb.state);
    state.borrow_mut().bus[Bus::A.index()]
        .bring_up
        .push_back(Err(BringUpFailure::Init("boom")));
    let mut rig = link_over(
        &state,
        |con| start_buses(ipmb, &addr::resolve(0), con),
        RamFlash::new(),
    );
    // start-up line: taken from the console before `link_over` cleared it, so look at the state.
    assert_eq!(
        rig.link.link().bus_state(Bus::A).status,
        LocalStatus::Undiagnosed
    );
    assert_eq!(rig.link.fault_state(Bus::A), (0, true));
    assert_eq!(
        rig.events().contains("bring_up B"),
        ipmc_defs::IPMB_BUS_B_ENABLE
    );
    state.borrow_mut().bus[Bus::A.index()].lines_high = true;
    // The same reason again after the wait: no new report. A new reason: one line.
    for (script, line) in [
        (BringUpFailure::Init("boom"), ""),
        (
            BringUpFailure::Bus(MasterError::SdaStuckLow),
            "IPMB-A: bus not usable: SDA stuck low; the bus stays disabled\r\n",
        ),
    ] {
        state.borrow_mut().bus[Bus::A.index()]
            .bring_up
            .push_back(Err(script));
        rig.link.service();
        rig.clock.advance(FAULT_CLEAR_MS);
        rig.link.service();
        assert_eq!(rig.text(), line);
    }
    // The recovery frees the bus: the target starts, the line says so, the fault clears.
    state.borrow_mut().bus[Bus::A.index()]
        .bring_up
        .push_back(Ok(Recovery::Recovered));
    rig.link.service();
    rig.clock.advance(FAULT_CLEAR_MS);
    rig.link.service();
    assert_eq!(
        rig.text(),
        "IPMB-A: SDA was held low; released by the recovery clocks\r\nIPMB-A fault cleared\r\n"
    );
    assert_eq!(rig.link.fault_state(Bus::A), (0, false));
    state.borrow_mut().push_frame(Bus::A, &[1, 2, 3], 0);
    rig.link.service();
    assert_eq!(received_order(&rig.link), [Bus::A]);
}

#[test]
fn start_buses_prints_a_failure_and_leaves_an_unusable_address_alone() {
    let ipmb = SimIpmb::default();
    let state = Rc::clone(&ipmb.state);
    state.borrow_mut().bus[Bus::A.index()]
        .bring_up
        .push_back(Err(BringUpFailure::Init("boom")));
    let mut con = SimSerial::roomy();
    let started = start_buses(ipmb, &addr::resolve(0), &mut con);
    assert!(started.rx_a.is_none());
    assert_eq!(started.fault_a, Some(BringUpFailure::Init("boom")));
    assert_eq!(
        con.take_text(),
        "IPMB-A: I2C init failed: boom; the bus stays disabled\r\n"
    );
    // GA = 0 is no address: nothing is started.
    let mut unusable = addr::resolve(0);
    unusable.valid = false;
    assert!(!unusable.usable());
    let ipmb = SimIpmb::default();
    let state = Rc::clone(&ipmb.state);
    let started = start_buses(ipmb, &unusable, &mut con);
    assert!(started.rx_a.is_none() && started.rx_b.is_none());
    assert!(state.borrow().events.is_empty());
}

#[test]
fn an_erase_waits_for_a_queued_frame_or_a_request_in_flight() {
    // A frame in a bus queue, then a request the link accepted and has not answered.
    for in_flight in [false, true] {
        let mut rig = rig_with(full_flash());
        let own = rig.link.link().own_address();
        if in_flight {
            assert!(rig.link.inject(Bus::A, &get_device_id(own, 1)).is_ok());
        } else {
            rig.bus.borrow_mut().push_frame(Bus::B, &[1, 2, 3], 0);
        }
        assert_eq!(rig.link.fru_restore(), Err(RestoreError::Busy));
        assert_eq!(rig.erases(), 0, "in flight {in_flight}");
        assert_eq!(
            rig.events(),
            "suspend A,suspend B,resume A,resume B",
            "in flight {in_flight}"
        );
        assert!(!rig.text().contains("erased"));
    }
}

#[test]
fn an_erase_pauses_both_slaves_and_reports_the_time() {
    let mut rig = rig_with(full_flash());
    // 250 000 counts per read at 1 000 counts per millisecond.
    rig.cycles.set_step(250_000);
    assert_eq!(rig.link.fru_restore(), Ok(()));
    assert_eq!(rig.erases(), 1);
    assert_eq!(rig.events(), "suspend A,suspend B,resume A,resume B");
    assert_eq!(
        rig.text(),
        "FRU log: erased the store sector at run time (IPMB paused 250 ms)\r\n"
    );
}

#[test]
fn a_faulted_bus_is_taken_off_without_waiting_for_idle() {
    let mut rig = rig_with(full_flash());
    {
        let mut state = rig.bus.borrow_mut();
        state.bus[Bus::A.index()].stats.bus_error = FAULT_STREAK;
        state.bus[Bus::A.index()].suspend_busy = 5;
    }
    rig.link.service();
    assert!(rig.link.fault_state(Bus::A).1);
    rig.bus.borrow_mut().events.clear();
    assert_eq!(rig.link.fru_restore(), Ok(()));
    assert_eq!(
        rig.events(),
        "suspend_faulted A,suspend B,resume A,resume B"
    );
}

#[test]
fn the_urgent_save_erases_with_a_busy_link_after_the_wait_and_says_so_once() {
    let mut rig = rig_with(full_flash());
    let own = rig.link.link().own_address();
    assert!(rig
        .link
        .set_unit_field(FruField::BoardSerial, "SN-1")
        .is_ok());
    rig.link.request_bootloader(Interface::Console);
    assert!(rig.link.inject(Bus::A, &get_device_id(own, 2)).is_ok());
    let line = std::format!(
        "FRU log: urgent save erases with the link still busy after {URGENT_LINK_WAIT_MS} ms\r\n"
    );
    // Inside the wait the erase is deferred: the slaves come off and go back, nothing is erased.
    rig.link.service_fru_log();
    rig.clock.advance(URGENT_LINK_WAIT_MS - 1);
    rig.link.service_fru_log();
    assert_eq!((rig.erases(), rig.text()), (0, String::new()));
    // Past it the erase runs once, and the line is printed once.
    rig.clock.advance(1);
    rig.link.service_fru_log();
    let text = rig.text();
    assert_eq!(rig.erases(), 1);
    assert_eq!(text.matches(line.as_str()).count(), 1, "{text}");
    assert!(text.contains("(sector erased first)"), "{text}");
    rig.link.service_fru_log();
    assert!(!rig.text().contains("urgent"));
}

#[test]
fn a_cold_reset_returns_its_restart_once_after_the_status_line() {
    // (the interface that asked for the bootloader, the line, the restart)
    let cases = [
        (None, "Cold Reset: restarting\r\n"),
        (
            Some(Interface::Console),
            "Cold Reset: restarting into the bootloader\r\n",
        ),
        (
            Some(Interface::App),
            "Cold Reset: restarting into the bootloader\r\n",
        ),
    ];
    for (boot, line) in cases {
        let mut rig = rig();
        match boot {
            Some(interface) => rig.link.request_bootloader(interface),
            None => rig.link.dispatch.pending_reset = Some(ResetRequest::Cold),
        }
        assert_eq!(rig.link.service(), Some(Restart { boot }));
        assert_eq!(rig.text(), line);
    }
}

#[test]
fn a_warm_reset_keeps_the_local_status_and_forgets_a_bootloader_request() {
    let mut rig = rig();
    rig.bus.borrow_mut().bus[Bus::A.index()].stats.bus_error = FAULT_STREAK;
    rig.link.service();
    let status = rig.link.link().bus_state(Bus::A).status;
    assert_ne!(status, LocalStatus::None);
    rig.text();
    rig.link.request_bootloader(Interface::Console);
    rig.link.dispatch.pending_reset = Some(ResetRequest::Warm);
    assert_eq!(rig.link.service(), None);
    assert_eq!(rig.text(), "Warm Reset: link state re-initialised\r\n");
    assert_eq!(rig.link.link().bus_state(Bus::A).status, status);
    // The bootloader request did not survive the Warm Reset: the next Cold Reset is plain.
    rig.link.dispatch.pending_reset = Some(ResetRequest::Cold);
    assert_eq!(rig.link.service(), Some(Restart { boot: None }));
}

#[test]
fn restart_flushes_keeps_the_sequence_and_writes_the_boot_request() {
    for boot in [None, Some(Interface::App)] {
        let rig = rig();
        let (mut con, mut app) = (SimSerial::roomy(), SimSerial::roomy());
        let (mut kept, mut req) = ([0u32; 4], [0u32; 4]);
        let wanted = rig.link.kept_state();
        let ended = catch_unwind(AssertUnwindSafe(|| {
            restart::<Sim>(
                &rig.link,
                &mut con,
                &mut app,
                &mut kept,
                &mut req,
                &mut SimSystem,
                Restart { boot },
            )
        }));
        assert!(ended.is_err_and(|payload| payload.is::<Reset>()));
        assert_eq!((con.flushes(), app.flushes()), (1, 1));
        let expected_req = boot.map_or([0u32; 4], request::encode);
        assert_eq!(req, expected_req);
        assert_eq!(
            crate::kept::kept_sequence(&mut kept, ResetCause::Software),
            wanted
        );
    }
}

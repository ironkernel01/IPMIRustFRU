// board_api.rs - what a caller of the emulator relies on: the refusals of `rx`, the failure
// choices of the simulated buses, the console and the board's clock.

use ipmc_core::ipmb::{self, Header, NETFN_APP};
use ipmc_core::link::{FrameDrop, TxPolicy};
use ipmc_core::queue::Bus;
use ipmc_core::trace::RecordingTrace;
use ipmc_emu::emulator::{EmuConfig, Emulator};
use ipmc_runtime::sim::IpmbState;

const OWN: u8 = 0x82;
const CM: u8 = 0x20;
const GET_DEVICE_ID: u8 = 0x01;

fn board(b_fitted: bool, b_enabled: bool) -> Emulator {
    Emulator::new(EmuConfig {
        b_fitted,
        b_enabled_at_start: b_enabled,
        tx_policy: TxPolicy::Arrival,
        // No event requests of a Tier-2/3 build among the frames these tests count.
        events_at_start: false,
        ..EmuConfig::from_defs()
    })
    .expect("emulator")
}

fn get_device_id(seq: u8) -> Vec<u8> {
    let header = Header {
        dest: OWN,
        netfn: NETFN_APP,
        dest_lun: 0,
        src: CM,
        seq,
        src_lun: 0,
        cmd: GET_DEVICE_ID,
    };
    let mut out = vec![0u8; ipmb::IPMB_MAX_LEN];
    let len = ipmb::encode_request(&header, &[], &mut out).unwrap();
    out.truncate(len);
    out
}

#[test]
fn rx_refuses_a_frame_the_board_would_lose() {
    let mut emu = board(false, false);
    let frame = get_device_id(1);
    assert_eq!(
        emu.rx(Bus::B, &frame, 0),
        Err(FrameDrop::BusUnavailable),
        "no target runs on IPMB-B in a build without it"
    );
    assert_eq!(emu.rx(Bus::A, &frame, 0), Ok(()));
    // A queue holds INBOX_MAX frames; nothing runs between these calls, so none is taken.
    for _ in 1..IpmbState::INBOX_MAX {
        assert_eq!(emu.rx(Bus::A, &frame, 0), Ok(()));
    }
    assert_eq!(emu.rx(Bus::A, &frame, 0), Err(FrameDrop::IngressFull));
}

#[test]
fn a_down_bus_fails_the_write_and_the_link_resends_on_the_other_bus() {
    let mut emu = board(true, true);
    let mut lines = RecordingTrace::default();
    emu.set_bus_down(Bus::A, true);
    emu.rx(Bus::A, &get_device_id(1), 10).unwrap();
    emu.poll(10, &mut lines);
    let sent: Vec<_> = emu.take_sent().into_iter().map(|t| (t.bus, t.ok)).collect();
    assert_eq!(sent, [(Bus::A, false), (Bus::B, true)]);
    // Repaired, the next response leaves on the bus it arrived on.
    emu.set_bus_down(Bus::A, false);
    emu.rx(Bus::A, &get_device_id(2), 20).unwrap();
    emu.poll(20, &mut lines);
    let sent: Vec<_> = emu.take_sent().into_iter().map(|t| (t.bus, t.ok)).collect();
    assert_eq!(sent, [(Bus::A, true)]);
}

#[test]
fn every_nth_write_over_both_buses_fails_and_the_count_goes_on() {
    let mut emu = board(true, true);
    let mut lines = RecordingTrace::default();
    emu.set_fail_every(3);
    let mut outcomes = Vec::new();
    for seq in 1..=4u8 {
        let now = u32::from(seq) * 300;
        emu.rx(Bus::A, &get_device_id(seq), now).unwrap();
        emu.poll(now, &mut lines);
        outcomes.extend(emu.take_sent().into_iter().map(|t| t.ok));
    }
    // Writes 1 and 2 pass, write 3 fails and its resend (write 4) passes, then write 5.
    assert_eq!(outcomes, [true, true, false, true, true]);
}

#[test]
fn the_console_line_runs_and_its_text_reaches_the_next_poll() {
    let mut emu = board(false, false);
    let mut lines = RecordingTrace::default();
    emu.console_line("SYS LED OFF");
    emu.poll(0, &mut lines);
    assert!(
        lines.lines.iter().any(|l| l.starts_with("SYS LED OFF")),
        "the echo of the command: {:?}",
        lines.lines
    );
    assert!(
        lines.lines.iter().any(|l| l.starts_with("event: power")),
        "the sensor events of the first pass: {:?}",
        lines.lines
    );
}

#[test]
fn the_board_clock_never_runs_back() {
    let mut emu = board(false, false);
    let mut lines = RecordingTrace::default();
    emu.poll(500, &mut lines);
    // An earlier time is raised to the latest one: the frame is stamped 500 ms and answered.
    emu.rx(Bus::A, &get_device_id(1), 400).unwrap();
    emu.poll(450, &mut lines);
    assert_eq!(emu.take_sent().len(), 1);
}

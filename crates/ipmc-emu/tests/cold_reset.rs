// cold_reset.rs - the emulator's Cold and Warm Reset: `Context::pending_reset` is applied by the
// shared `ResetGate` when the response has left and no accepted FRU write is unsaved, or
// IPMC_RESET_WAIT_MS after the request when it cannot. A Cold Reset restores the FRU image from
// the flash copy, continues the request sequence numbers and restarts the payload power
// controller; a Warm Reset rebuilds the link, continues the sequence numbers and keeps the rest.
// The hand-offs of a handler (payload power request, payload reset pulse) are served here too.

use ipmc_core::dispatch::PowerRequest;
use ipmc_core::dispatch::{Context, ResetRequest};
use ipmc_core::ext::Extension;
use ipmc_core::ipmb::{self, Frame, Header, NETFN_APP, NETFN_SE, NETFN_STORAGE};
use ipmc_core::queue::Bus;
use ipmc_core::requester::{Request, Requester};
use ipmc_core::sensor::{PayloadMode, PowerState, TransitionStatus};
use ipmc_core::trace::Trace;
use ipmc_emu::emulator::{EmuConfig, Emulator};
use std::cell::Cell;

const OWN: u8 = 0x82;
const CM: u8 = 0x20;
const GET_DEVICE_ID: u8 = 0x01;
/// A command no Tier-1 row owns: the test extension answers it with a Cold Reset request.
const FAKE_RESET_CMD: u8 = 0x7E;

/// Commands no Tier-1 row owns: Warm Reset; set the kept state (payload mode 3, settings 01h,
/// events of sensor 2 off); payload power on; payload power off; payload reset.
const FAKE_WARM_CMD: u8 = 0x7C;
const FAKE_KEEP_CMD: u8 = 0x7B;
const FAKE_POWER_ON_CMD: u8 = 0x7A;
const FAKE_POWER_OFF_CMD: u8 = 0x79;
const FAKE_PAYLOAD_RESET_CMD: u8 = 0x78;
thread_local! {
    /// `Extension::warm_reset` calls of the test extension, per test thread.
    static WARM_CALLS: Cell<u32> = const { Cell::new(0) };
}

/// A command no Tier-1 row owns: the test extension sends one Platform Event request after it.
const FAKE_SEND_CMD: u8 = 0x7D;
const WRITE_FRU_DATA: u8 = 0x12;
const READ_FRU_DATA: u8 = 0x11;
const PLATFORM_EVENT: u8 = 0x02;

/// FakeReset -- answers FAKE_RESET_CMD by setting `pending_reset`, as the Tier-2/3 Cold Reset
/// handler does after validating its request, and FAKE_SEND_CMD by sending one Platform Event
/// request through the requester. With `locked` it refuses payload power, as a Tier-3 extension
/// does while the FRU is activation-locked (`Extension::power_on_allowed`).
#[derive(Default)]
struct FakeReset {
    send: bool,
    locked: bool,
}

impl Extension for FakeReset {
    fn command(
        &mut self,
        ctx: &mut Context,
        req: &Frame,
        _out: &mut [u8],
    ) -> Option<(&'static str, Result<usize, u8>)> {
        if req.header.netfn != NETFN_APP {
            return None;
        }
        match req.header.cmd {
            FAKE_RESET_CMD => {
                ctx.pending_reset = Some(ResetRequest::Cold);
                Some(("Fake Cold Reset", Ok(0)))
            }
            FAKE_SEND_CMD => {
                self.send = true;
                Some(("Fake Send", Ok(0)))
            }
            FAKE_WARM_CMD => {
                ctx.pending_reset = Some(ResetRequest::Warm);
                Some(("Fake Warm Reset", Ok(0)))
            }
            FAKE_KEEP_CMD => {
                ctx.payload_mode = PayloadMode::new(3, 0, TransitionStatus::Complete).unwrap();
                ctx.set_nv_settings(0b01);
                if let Some(events) = ctx.event_enables.get_mut(2) {
                    events.enabled = false;
                }
                Some(("Fake Keep", Ok(0)))
            }
            FAKE_POWER_ON_CMD => {
                ctx.pending_power = Some(PowerRequest::On);
                Some(("Fake Power On", Ok(0)))
            }
            FAKE_POWER_OFF_CMD => {
                ctx.pending_power = Some(PowerRequest::Off);
                Some(("Fake Power Off", Ok(0)))
            }
            FAKE_PAYLOAD_RESET_CMD => {
                ctx.pending_payload_reset = true;
                Some(("Fake Payload Reset", Ok(0)))
            }
            _ => None,
        }
    }

    fn warm_reset(&mut self) {
        WARM_CALLS.with(|calls| calls.set(calls.get() + 1));
    }

    fn power_on_allowed(&self, _ctx: &Context) -> bool {
        !self.locked
    }

    fn poll(
        &mut self,
        _ctx: &mut Context,
        requester: &mut Requester,
        now_ms: u32,
        _trace: &mut dyn Trace,
    ) {
        let _ = requester.take_completion();
        if self.send {
            let request = Request {
                dest_addr8: CM,
                dest_lun: 0,
                netfn: NETFN_SE,
                cmd: PLATFORM_EVENT,
                data: &[0x04, 0xF0, 0x00, 0x6F, 0xA1, 0x00, 0x00],
            };
            self.send = requester.submit(&request, now_ms).is_err();
        }
    }
}

fn make() -> Box<dyn Extension> {
    Box::new(FakeReset::default())
}

fn make_locked() -> Box<dyn Extension> {
    Box::new(FakeReset {
        locked: true,
        ..FakeReset::default()
    })
}

/// Lines -- collects the trace lines.
#[derive(Default)]
struct Lines(Vec<String>);

impl Trace for Lines {
    fn line(&mut self, args: core::fmt::Arguments) {
        self.0.push(std::fmt::format(args));
    }
}

impl Lines {
    fn resets(&self) -> usize {
        self.count("IPMC cold reset")
    }

    fn count(&self, line: &str) -> usize {
        self.0.iter().filter(|l| *l == line).count()
    }
}

fn board() -> Emulator {
    let config = EmuConfig {
        b_enabled_at_start: false,
        ..EmuConfig::from_defs()
    };
    Emulator::new(config)
        .expect("emulator")
        .with_extension(make)
}

fn request(cmd: u8, seq: u8) -> Vec<u8> {
    request_with(NETFN_APP, cmd, seq, &[])
}

fn request_with(netfn: u8, cmd: u8, seq: u8, data: &[u8]) -> Vec<u8> {
    // The Chassis Manager's request to the board: the base header with the fields that vary.
    const BASE: Header = Header {
        dest: OWN,
        netfn: NETFN_APP,
        dest_lun: 0,
        src: CM,
        seq: 0,
        src_lun: 0,
        cmd: 0,
    };
    let header = Header {
        netfn,
        seq,
        cmd,
        ..BASE
    };
    let mut out = vec![0u8; ipmb::IPMB_MAX_LEN];
    let len = ipmb::encode_request(&header, data, &mut out).unwrap();
    out.truncate(len);
    out
}

/// exchange -- one request frame in at `now_ms`, one poll, the response out and reported sent 1 ms
/// later; returns the response bytes.
fn exchange(emu: &mut Emulator, frame: &[u8], now_ms: u32, lines: &mut Lines) -> Vec<u8> {
    emu.rx(Bus::A, frame, now_ms).unwrap();
    assert_eq!(emu.poll(now_ms, lines), 1);
    let (bus, bytes) = emu.next_tx().map(|(b, f)| (b, f.to_vec())).unwrap();
    emu.tx_done(bus, true, now_ms + 1, lines).unwrap();
    bytes
}

#[test]
fn the_reset_happens_after_the_response_left_and_the_board_answers_again() {
    let mut emu = board();
    let mut lines = Lines::default();
    let before = exchange(&mut emu, &request(GET_DEVICE_ID, 1), 0, &mut lines);
    assert_eq!(emu.link().counters(Bus::A).rx_ok, 1);
    let fru_before = emu.context().fru.bytes().to_vec();

    // The reset request is taken; its response waits, so the board has not reset yet.
    emu.rx(Bus::A, &request(FAKE_RESET_CMD, 2), 10).unwrap();
    assert_eq!(emu.poll(10, &mut lines), 1);
    assert_eq!(emu.context().pending_reset, Some(ResetRequest::Cold));
    emu.poll(11, &mut lines);
    assert_eq!(emu.context().pending_reset, Some(ResetRequest::Cold));
    assert_eq!(lines.resets(), 0, "not before the response has left");

    // The response leaves; the next poll resets.
    let (bus, response) = emu.next_tx().map(|(b, f)| (b, f.to_vec())).unwrap();
    assert_eq!(ipmb::parse(&response).unwrap().completion_code, Some(0));
    emu.tx_done(bus, true, 12, &mut lines).unwrap();
    assert_eq!(emu.context().pending_reset, Some(ResetRequest::Cold));
    emu.poll(13, &mut lines);
    assert_eq!(lines.resets(), 1);
    assert_eq!(emu.context().pending_reset, None);
    assert_eq!(
        emu.link().counters(Bus::A).rx_ok,
        0,
        "the link restarted as at start-up"
    );
    assert_eq!(emu.context().fru.bytes(), fru_before.as_slice());

    // After the reset the board answers the same request with the same response, and the
    // extension is the one the factory builds again.
    let after = exchange(&mut emu, &request(GET_DEVICE_ID, 1), 20, &mut lines);
    assert_eq!(after, before);
    emu.rx(Bus::A, &request(FAKE_RESET_CMD, 3), 30).unwrap();
    emu.poll(30, &mut lines);
    assert_eq!(emu.context().pending_reset, Some(ResetRequest::Cold));
}

#[test]
fn the_reset_happens_after_the_wait_when_the_response_cannot_leave() {
    let wait = ipmc_defs::IPMC_RESET_WAIT_MS;
    let mut emu = board();
    let mut lines = Lines::default();
    emu.rx(Bus::A, &request(FAKE_RESET_CMD, 1), 100).unwrap();
    assert_eq!(emu.poll(100, &mut lines), 1);
    // No driver takes the response (no bus can send): the egress queue stays occupied.
    emu.poll(100 + wait - 1, &mut lines);
    assert_eq!(lines.resets(), 0);
    assert_eq!(emu.context().pending_reset, Some(ResetRequest::Cold));
    emu.poll(100 + wait, &mut lines);
    assert_eq!(lines.resets(), 1);
    assert_eq!(emu.context().pending_reset, None);
    assert!(
        emu.next_tx().is_none(),
        "the queued response went with the reset"
    );
}

/// read_last_byte -- the last FRU byte as the Chassis Manager reads it (Read FRU Data).
fn read_last_byte(emu: &mut Emulator, seq: u8, now_ms: u32, lines: &mut Lines) -> u8 {
    let [last, last_hi] = u16::try_from(emu.context().fru.bytes().len() - 1)
        .unwrap()
        .to_le_bytes();
    let frame = request_with(NETFN_STORAGE, READ_FRU_DATA, seq, &[0, last, last_hi, 1]);
    let response = exchange(emu, &frame, now_ms, lines);
    let parsed = ipmb::parse(&response).unwrap();
    assert_eq!(parsed.completion_code, Some(0));
    assert_eq!(parsed.data[0], 1, "one byte read");
    parsed.data[1]
}

#[test]
fn an_accepted_fru_write_survives_a_cold_reset_at_once() {
    let mut emu = board();
    let mut lines = Lines::default();
    let [last, last_hi] = u16::try_from(emu.context().fru.bytes().len() - 1)
        .unwrap()
        .to_le_bytes();
    let before = read_last_byte(&mut emu, 1, 0, &mut lines);
    let marker = !before;
    // Write FRU Data, answered 00h; the save delay (IPMC_FRU_PERSIST_DELAY_MS) has not passed.
    let write = request_with(
        NETFN_STORAGE,
        WRITE_FRU_DATA,
        2,
        &[0, last, last_hi, marker],
    );
    let response = exchange(&mut emu, &write, 10, &mut lines);
    assert_eq!(ipmb::parse(&response).unwrap().completion_code, Some(0));
    // Cold Reset at once: the response leaves, the unsaved write is saved, then the board resets.
    emu.rx(Bus::A, &request(FAKE_RESET_CMD, 3), 20).unwrap();
    assert_eq!(emu.poll(20, &mut lines), 1);
    let (bus, _) = emu.next_tx().map(|(b, f)| (b, f.to_vec())).unwrap();
    emu.tx_done(bus, true, 21, &mut lines).unwrap();
    emu.poll(22, &mut lines);
    assert_eq!(lines.resets(), 1);
    assert_eq!(read_last_byte(&mut emu, 1, 30, &mut lines), marker);
}

#[test]
fn a_cold_reset_restores_the_fru_image_from_the_flash_copy() {
    // A write is saved IPMC_FRU_PERSIST_DELAY_MS after it was seen; a reset after that restores
    // it from the copy, and the RAM store is not what the reset keeps.
    let mut emu = board();
    let mut lines = Lines::default();
    let [last, last_hi] = u16::try_from(emu.context().fru.bytes().len() - 1)
        .unwrap()
        .to_le_bytes();
    let marker = !read_last_byte(&mut emu, 1, 0, &mut lines);
    let write = request_with(
        NETFN_STORAGE,
        WRITE_FRU_DATA,
        2,
        &[0, last, last_hi, marker],
    );
    exchange(&mut emu, &write, 10, &mut lines);
    emu.poll(10 + ipmc_defs::IPMC_FRU_PERSIST_DELAY_MS, &mut lines);
    exchange(&mut emu, &request(FAKE_RESET_CMD, 3), 2000, &mut lines);
    emu.poll(2002, &mut lines);
    assert_eq!(lines.resets(), 1);
    assert_eq!(read_last_byte(&mut emu, 1, 3000, &mut lines), marker);
}

/// event_round -- asks the fake extension for one Platform Event (Chassis Manager request
/// number `cm_seq`) at `now_ms`, takes the request frame it sends, answers 00h as the Event
/// Receiver; returns the rqSeq of the request.
fn event_round(emu: &mut Emulator, cm_seq: u8, now_ms: u32, lines: &mut Lines) -> u8 {
    exchange(emu, &request(FAKE_SEND_CMD, cm_seq), now_ms, lines);
    emu.poll(now_ms + 2, lines);
    let (bus, frame) = emu.next_tx().map(|(b, f)| (b, f.to_vec())).unwrap();
    emu.tx_done(bus, true, now_ms + 3, lines).unwrap();
    let sent = ipmb::parse(&frame).unwrap();
    let header = ipmb::response_header_for(&sent).unwrap();
    let mut buf = [0u8; ipmb::IPMB_MAX_LEN];
    let n = ipmb::encode_response(&header, 0, &[], &mut buf).unwrap();
    emu.rx(bus, &buf[..n], now_ms + 4).unwrap();
    emu.poll(now_ms + 5, lines);
    sent.header.seq
}

#[test]
fn no_sequence_number_sent_inside_t9_is_used_again_after_a_cold_reset() {
    if !ipmc_core::link::LinkConfig::from_defs().requests_allowed {
        return; // a Tier-1 build sends no request
    }
    // 64 events 50 ms apart wrap the 6-bit counter inside T9, then a Cold Reset at 3.4 s: the
    // counter alone would name sequence 0, which went out 3.4 s ago.
    let (spacing_ms, events) = (50u32, 64u32);
    let mut emu = board();
    let mut lines = Lines::default();
    for i in 0..events {
        let cm_seq = u8::try_from(i).unwrap();
        assert_eq!(
            event_round(&mut emu, cm_seq, i * spacing_ms, &mut lines),
            u8::try_from(i).unwrap()
        );
    }
    let reset_ms = events * spacing_ms + 200;
    exchange(&mut emu, &request(FAKE_RESET_CMD, 0), reset_ms, &mut lines);
    emu.poll(reset_ms + 2, &mut lines);
    assert_eq!(lines.resets(), 1);
    // The fake extension asks again each poll until the requester takes the event.
    exchange(
        &mut emu,
        &request(FAKE_SEND_CMD, 62),
        reset_ms + 100,
        &mut lines,
    );
    let mut first = None;
    let mut now_ms = reset_ms + 110;
    while first.is_none() && now_ms < reset_ms + 3 * ipmc_core::requester::SEQ_EXPIRY_MS {
        emu.poll(now_ms, &mut lines);
        if let Some((bus, frame)) = emu.next_tx().map(|(b, f)| (b, f.to_vec())) {
            emu.tx_done(bus, true, now_ms + 1, &mut lines).unwrap();
            first = Some((now_ms, ipmb::parse(&frame).unwrap().header.seq));
        }
        now_ms += 10;
    }
    let (sent_ms, seq) = first.expect("the event goes out once its sequence number is free");
    let old_ms = u32::from(seq) * spacing_ms;
    assert!(
        sent_ms - old_ms >= ipmc_core::requester::SEQ_EXPIRY_MS,
        "sequence {seq} went out at {old_ms} ms and again at {sent_ms} ms"
    );
}

#[test]
fn a_cold_reset_restarts_the_payload_power_as_a_boot_does() {
    if ipmc_defs::IPMC_POWER_AT_BOOT != 1 {
        return;
    }
    let mut emu = board();
    let mut lines = Lines::default();
    for now_ms in (0..=3000).step_by(500) {
        emu.poll(now_ms, &mut lines);
    }
    assert_eq!(emu.payload_power(), PowerState::On);
    exchange(&mut emu, &request(FAKE_RESET_CMD, 1), 3000, &mut lines);
    emu.poll(3002, &mut lines);
    assert_eq!(lines.resets(), 1);
    assert_eq!(
        emu.payload_power(),
        PowerState::PowerOnRequested,
        "the processor reset released PWR_EN; the boot requests power again"
    );
    for now_ms in (3500..=6500).step_by(500) {
        emu.poll(now_ms, &mut lines);
    }
    assert_eq!(emu.payload_power(), PowerState::On);
}

/// run_to_power_on -- polls the board until the payload power is On (power requested at boot).
fn run_to_power_on(emu: &mut Emulator, lines: &mut Lines) {
    for now_ms in (0..=3000).step_by(500) {
        emu.poll(now_ms, lines);
    }
}

#[test]
fn a_warm_reset_rebuilds_the_link_and_keeps_everything_else() {
    if ipmc_defs::IPMC_POWER_AT_BOOT != 1 {
        return;
    }
    let mut emu = board();
    let mut lines = Lines::default();
    run_to_power_on(&mut emu, &mut lines);
    assert_eq!(emu.payload_power(), PowerState::On);
    exchange(&mut emu, &request(FAKE_KEEP_CMD, 1), 3000, &mut lines);
    let kept = emu.context().fru_state.state();
    let (fru, events, sdr_count) = (
        emu.context().fru.bytes().to_vec(),
        emu.context().event_enables,
        emu.context().sdr.record_count(),
    );
    assert!(
        !events.get(2).unwrap().enabled,
        "the test changed the enables"
    );
    assert_eq!(emu.link().counters(Bus::A).rx_ok, 1);
    // The Warm Reset request is taken; it is applied once the response has left, as a Cold Reset.
    emu.rx(Bus::A, &request(FAKE_WARM_CMD, 2), 3010).unwrap();
    assert_eq!(emu.poll(3010, &mut lines), 1);
    emu.poll(3011, &mut lines);
    assert_eq!(emu.context().pending_reset, Some(ResetRequest::Warm));
    assert_eq!(
        lines.count("IPMC warm reset"),
        0,
        "not before the response left"
    );
    let (bus, _) = emu.next_tx().map(|(b, f)| (b, f.to_vec())).unwrap();
    emu.tx_done(bus, true, 3012, &mut lines).unwrap();
    emu.poll(3013, &mut lines);
    assert_eq!(lines.count("IPMC warm reset"), 1);
    assert_eq!(lines.resets(), 0, "a Warm Reset is not a Cold Reset");
    assert_eq!(emu.context().pending_reset, None);
    assert_eq!(WARM_CALLS.with(Cell::get), 1, "the extension is told once");
    // The link restarted; the rest is kept: payload power (no new power-on), FRU state, FRU
    // store, SDR, event enables, payload mode, the settings byte.
    assert_eq!(emu.link().counters(Bus::A).rx_ok, 0);
    assert_eq!(emu.payload_power(), PowerState::On);
    assert_eq!(emu.context().fru_state.state(), kept);
    assert_eq!(emu.context().fru.bytes(), fru.as_slice());
    assert_eq!(emu.context().event_enables, events);
    assert_eq!(emu.context().sdr.record_count(), sdr_count);
    assert_eq!(emu.context().nv_settings(), 0b01);
    assert_eq!(emu.context().payload_mode.current(), 3);
    // The board answers again on the restarted link.
    exchange(&mut emu, &request(GET_DEVICE_ID, 1), 3100, &mut lines);
    assert_eq!(emu.link().counters(Bus::A).rx_ok, 1);
}

#[test]
fn the_request_sequence_continues_across_a_cold_and_a_warm_reset() {
    if !ipmc_core::link::LinkConfig::from_defs().requests_allowed {
        return; // a Tier-1 build sends no request
    }
    // (command that asks for the reset, the trace line of its application)
    for (command, applied) in [
        (FAKE_RESET_CMD, "IPMC cold reset"),
        (FAKE_WARM_CMD, "IPMC warm reset"),
    ] {
        let mut emu = board();
        let mut lines = Lines::default();
        assert_eq!(event_round(&mut emu, 1, 0, &mut lines), 0);
        assert_eq!(event_round(&mut emu, 2, 100, &mut lines), 1);
        exchange(&mut emu, &request(command, 3), 200, &mut lines);
        emu.poll(202, &mut lines);
        assert_eq!(lines.count(applied), 1, "{applied}");
        assert_eq!(
            event_round(&mut emu, 1, 300, &mut lines),
            2,
            "the counter goes on, inside T9 no value repeats: {applied}"
        );
    }
}

#[test]
fn the_payload_power_and_reset_hand_offs_are_served_by_the_owner() {
    let mut emu = board();
    let mut lines = Lines::default();
    run_to_power_on(&mut emu, &mut lines);
    exchange(&mut emu, &request(FAKE_POWER_OFF_CMD, 1), 3000, &mut lines);
    assert_eq!(emu.payload_power(), PowerState::Off);
    assert_eq!(emu.context().pending_power, None, "the request is consumed");
    // The Off request is sampled at once (M4 to M6); a power-on is allowed again once the
    // shutdown has completed (M1), which the next periodic sample (IPMC_SENSOR_PERIOD_MS after the one the
    // request took) sees.
    emu.poll(4000, &mut lines);
    exchange(&mut emu, &request(FAKE_POWER_ON_CMD, 2), 4100, &mut lines);
    assert_eq!(emu.payload_power(), PowerState::PowerOnRequested);
    exchange(
        &mut emu,
        &request(FAKE_PAYLOAD_RESET_CMD, 3),
        4200,
        &mut lines,
    );
    let pulse = format!(
        "payload reset pulse {} ms",
        ipmc_defs::IPMC_PAYLOAD_RESET_PULSE_MS
    );
    assert_eq!(lines.count(&pulse), 1);
    assert!(!emu.context().pending_payload_reset);
}

#[test]
fn a_locked_extension_keeps_the_power_on_request_from_the_payload() {
    let mut emu = Emulator::new(EmuConfig {
        b_enabled_at_start: false,
        ..EmuConfig::from_defs()
    })
    .expect("emulator")
    .with_extension(make_locked);
    let mut lines = Lines::default();
    run_to_power_on(&mut emu, &mut lines);
    exchange(&mut emu, &request(FAKE_POWER_OFF_CMD, 1), 3000, &mut lines);
    assert_eq!(emu.payload_power(), PowerState::Off);
    // The request is consumed, refused with one trace line, and the payload stays off (the
    // shutdown the Off request began is complete by then: M1).
    emu.poll(4000, &mut lines);
    exchange(&mut emu, &request(FAKE_POWER_ON_CMD, 2), 4100, &mut lines);
    assert_eq!(emu.context().pending_power, None);
    assert_eq!(emu.payload_power(), PowerState::Off);
    assert_eq!(
        lines
            .0
            .iter()
            .filter(|l| l.starts_with("power on request refused: FRU is activation-locked"))
            .count(),
        1
    );
}

#[test]
fn the_payload_supply_follows_pwr_en_so_a_shutdown_can_finish() {
    if ipmc_defs::IPMC_POWER_AT_BOOT != 1 {
        return;
    }
    let mut emu = board();
    let mut lines = Lines::default();
    run_to_power_on(&mut emu, &mut lines);
    assert!(!emu.context().sensors.payload_shut_down, "PGOOD is high");
    exchange(&mut emu, &request(FAKE_POWER_OFF_CMD, 1), 3000, &mut lines);
    for now_ms in (3500..=6000).step_by(500) {
        emu.poll(now_ms, &mut lines);
    }
    // PWR_EN released and PGOOD low: the shutdown is complete (Rule 4.1.5.4.5-1).
    assert!(emu.context().sensors.payload_shut_down);
}

#[test]
fn a_power_cycle_keeps_the_flash_log_and_a_default_lock_holds_the_payload_off() {
    let config = EmuConfig {
        b_enabled_at_start: false,
        ..EmuConfig::from_defs()
    };
    let mut lines = Lines::default();
    let mut emu = board();
    exchange(&mut emu, &request(FAKE_KEEP_CMD, 1), 0, &mut lines);
    assert_eq!(emu.context().nv_settings(), 0b01);
    // The settings change is still inside the persist delay; the emulator's power cycle takes the
    // save with it, and the new board starts with the stored settings.
    let flash = emu.flash();
    let fresh = Emulator::new_with_flash(config, flash).expect("emulator");
    assert_eq!(fresh.context().nv_settings(), 0b01);
    assert_eq!(
        flash,
        Emulator::new_with_flash(config, flash).unwrap().flash()
    );
    // At Tier-3 the stored default-activation lock keeps the boot power-on request away from the
    // payload (the power-on hook of the extension); below it the payload is requested as ever.
    let on = fresh.payload_power() != PowerState::Off;
    assert_eq!(
        on,
        ipmc_defs::IPMC_TIER < 3 && ipmc_defs::IPMC_POWER_AT_BOOT == 1
    );
    // A board that never saved starts from the built image and settings 00h.
    let erased = Emulator::new(config).expect("emulator");
    assert_eq!(erased.context().nv_settings(), 0);
}

// replay_runs.rs - the capture replayed through the emulator under the four configurations of
// PLAN.md section 3.5, through the library.

mod common;
use common::load;
use ipmc_core::fru_state::State;
use ipmc_core::ipmb;
use ipmc_core::link::{IpmbBPowerUp, TxPolicy};
use ipmc_core::queue::Bus;
use ipmc_core::trace::{NoTrace, Trace};
use ipmc_emu::emulator::{EmuConfig, Emulator};
use ipmc_emu::replay::{is_request_frame, replay, ReplayOptions, ReplayReport};
use ipmc_emu::trace::Hex;
use ipmc_emu::vectors::{self, Vector};

/// Requests to the IPMC in the capture (GA 1, 82h).
const REQUESTS: u32 = 309;

fn config(b_fitted: bool, b_enabled: bool, policy: TxPolicy) -> EmuConfig {
    EmuConfig {
        b_fitted,
        b_enabled_at_start: b_enabled,
        tx_policy: policy,
        ..EmuConfig::from_defs()
    }
}

fn run(config: EmuConfig, fail_every: u32) -> (ReplayReport, Vec<Vector>) {
    let vectors = load();
    let mut emu = Emulator::new(config).expect("emulator");
    let report = replay(
        &mut emu,
        &vectors,
        &ReplayOptions { fail_every },
        &mut NoTrace,
    );
    (report, vectors)
}

/// Lines -- collects the trace lines of a replay.
struct Lines(Vec<String>);

impl Trace for Lines {
    fn line(&mut self, args: core::fmt::Arguments) {
        self.0.push(std::fmt::format(args));
    }
}

/// tx_frames -- (bus, succeeded, bytes) of every transmission of a traced replay, in order.
fn tx_frames(config: EmuConfig, fail_every: u32) -> Vec<(Bus, bool, Vec<u8>)> {
    let vectors = load();
    let mut emu = Emulator::new(config).expect("emulator");
    let mut lines = Lines(Vec::new());
    replay(
        &mut emu,
        &vectors,
        &ReplayOptions { fail_every },
        &mut lines,
    );
    lines
        .0
        .iter()
        .filter_map(|l| l.strip_prefix("tx IPMB-"))
        .map(|l| {
            let bus = if l.starts_with('A') { Bus::A } else { Bus::B };
            let (_, hex) = l.split_once(": ").expect("tx line has the frame");
            let bytes = vectors::frame_bytes(hex.split_whitespace()).expect("frame hex");
            (bus, l[2..].starts_with("ok"), bytes)
        })
        .collect()
}

/// tx_buses -- (bus, succeeded) of every response transmission of a traced replay, in order: the
/// IPMC's own requests (Tier 2 and 3) are left out.
fn tx_buses(config: EmuConfig, fail_every: u32) -> Vec<(Bus, bool)> {
    tx_frames(config, fail_every)
        .into_iter()
        .filter(|(_, _, bytes)| !is_request_frame(bytes))
        .map(|(bus, ok, _)| (bus, ok))
        .collect()
}

/// tier1 -- the build is a Tier-1 IPMC: it sends no request of its own, and the replay numbers
/// below are the Tier-1 ones. At Tier 2 and 3 the tests assert the structure and print numbers.
fn tier1() -> bool {
    ipmc_defs::IPMC_TIER == 1
}

/// tier1_numbers -- the Tier-1 replay numbers of the capture under the capture's own buses.
fn tier1_numbers(r: &ReplayReport) {
    // Match 44: Reserve Device SDR Repository 40 (the IDs run 1, 2, 3 ... as on the captured
    // board), Read FRU Data 3 (the image bytes agree in those ranges: the captured chunks that
    // are all 00h meet our 00h padding; the built areas hold room for every field, so the areas
    // and the MultiRecord area sit elsewhere and most chunks differ) and Set IPMB State 1.
    assert_eq!(r.totals.matched, 44);
    // SameCc 61: both answer 00h, but identity (Get Device ID), FRU image (Read FRU Data 12,
    // Inventory Area Info), SDR content (Get Device SDR 35, SDR Info), the sensor readings (8,
    // the emulated sensors differ), the VSO capabilities and the locator record ID differ from
    // the captured board.
    assert_eq!(r.totals.same_cc, 61);
    // CcDiff 115: Get Device SDR requests for chunks (15, C9h) or records (100, CBh) this IPMC's
    // 14-record repository (7 compact records, 6 analog Full records and the locator) does not
    // have; the captured board answered 00h. Before the analog sensors (8 records) it was 42 and
    // 134 (10 and 124).
    assert_eq!(r.totals.cc_diff, 115);
    // NotImplemented 89: Tier-1 scope returns C1h (PLAN.md section 4): Set Event Receiver and
    // Set Sensor Event Enable (4 + 78), Set SEL Time (1), Set FRU Activation (3), FRU Control
    // Capabilities (1), Get Mandatory Sensor Numbers (2).
    assert_eq!(r.totals.not_implemented, 89);
}

#[test]
fn arrival_policy_with_both_buses_matches_the_captures_own_buses() {
    if common::capture_missing() {
        return;
    }
    let (r, _) = run(config(true, true, TxPolicy::Arrival), 0);
    assert!(r.passed());
    assert_eq!(
        (r.requests, r.answered, r.totals.unanswered),
        (REQUESTS, REQUESTS, 0)
    );
    assert_eq!(r.bus_differs, 0);
    assert_eq!(r.malformed, 0);
    assert_eq!(r.totals.no_capture, 0);
    assert_eq!(r.totals.total(), REQUESTS);
    if tier1() {
        assert_eq!((r.ipmc_requests, r.events), (0, 0));
        tier1_numbers(&r);
    } else {
        // The extension changes the answers (more commands implemented, event enables), so only
        // the structure is asserted; the numbers are printed.
        println!("{}", r.summary_line());
        assert!(r.ipmc_requests > 0, "the IPMC sends its own requests");
        assert!(r.events > 0 && r.events <= r.ipmc_requests);
    }
    // Rows are named from COMMANDS only; a command the table does not hold reads as such.
    for row in &r.rows {
        let in_table = ipmc_core::commands::find(row.netfn, row.cmd).is_some();
        assert_eq!(
            row.name == "(not implemented)",
            !in_table,
            "{:02X}h/{:02X}h",
            row.netfn,
            row.cmd
        );
        // At Tier 2 and 3 the extension answers commands the COMMANDS table does not hold.
        if !in_table && tier1() {
            assert_eq!(row.counts.not_implemented, row.counts.total());
        }
    }
}

#[test]
fn alternate_policy_answers_every_request_and_alternates_buses() {
    if common::capture_missing() {
        return;
    }
    let policy = config(true, true, TxPolicy::Alternate);
    let (r, _) = run(policy, 0);
    assert!(r.passed());
    assert_eq!((r.answered, r.totals.unanswered), (REQUESTS, 0));
    let tx = tx_buses(policy, 0);
    assert_eq!(tx.len(), REQUESTS as usize);
    assert!(tx.iter().all(|(_, ok)| *ok));
    if tier1() {
        for (i, (bus, _)) in tx.iter().enumerate() {
            let expected = if i % 2 == 0 { Bus::A } else { Bus::B };
            assert_eq!(*bus, expected, "response {i}");
        }
    } else {
        // The IPMC's own requests take turns on the buses too, so the responses alone need not
        // strictly alternate; both buses carry them.
        println!(
            "responses on A: {}",
            tx.iter().filter(|t| t.0 == Bus::A).count()
        );
        assert!(tx.iter().any(|t| t.0 == Bus::A) && tx.iter().any(|t| t.0 == Bus::B));
    }
}

#[test]
fn failed_transmissions_are_resent_on_the_other_bus() {
    if common::capture_missing() {
        return;
    }
    let policy = config(true, true, TxPolicy::Alternate);
    let (r, _) = run(policy, 5);
    assert!(r.passed());
    assert_eq!(r.answered, REQUESTS);
    assert!(r.resends > 0);
    assert_eq!(r.resends_other_bus, r.resends);
    // Every failed transmission is followed by a transmission on the other bus.
    let tx = tx_buses(policy, 5);
    let failed = tx.iter().filter(|(_, ok)| !ok).count();
    assert_eq!(failed as u32, r.resends);
    for pair in tx.windows(2) {
        if !pair[0].1 {
            assert_ne!(pair[0].0, pair[1].0, "resend on the failed bus");
        }
    }
    let link_resends: u32 = r.link.buses.iter().map(|b| b.resends).sum();
    assert_eq!(link_resends, r.resends);
}

#[test]
fn without_ipmb_b_the_b_requests_are_not_fitted_not_unanswered() {
    if common::capture_missing() {
        return;
    }
    let (r, vectors) = run(config(false, false, TxPolicy::Alternate), 0);
    assert!(r.passed());
    let b_requests = vectors
        .iter()
        .filter(|v| v.bus == Bus::B)
        .filter_map(|v| ipmb::parse(&v.bytes).ok().map(|f| f.header))
        .filter(|h| !h.is_response() && h.dest == 0x82)
        .count() as u32;
    assert_eq!(b_requests, 155);
    assert_eq!(r.totals.bus_not_fitted, b_requests);
    assert_eq!(r.totals.unanswered, 0);
    assert_eq!(r.answered, REQUESTS - b_requests);
    assert_eq!(r.link.buses[1].unavailable, b_requests);
    let text = r.to_string();
    assert!(text.contains("requests on IPMB-B, not fitted in this build: 155"));
    assert!(text.trim_end().ends_with("PASS"));
}

/// IPMB-B fitted and not enabled at start. The capture's only Set IPMB State (enabling IPMB-B)
/// arrives on IPMB-B: Strict drops it with the 155 requests on IPMB-B, so all stay unanswered;
/// NonStrict answers the first 110 in the power-up window, and the Set IPMB State ends the window
/// and enables IPMB-B for the other 45.
#[test]
fn a_disabled_but_fitted_bus_depends_on_the_power_up_behaviour() {
    if common::capture_missing() {
        return;
    }
    // (behaviour, unanswered, dropped as unavailable, accepted in the window)
    let cases = [
        (IpmbBPowerUp::Strict, 155, 155, 0),
        (IpmbBPowerUp::NonStrict, 0, 0, 110),
    ];
    for (power_up, unanswered, unavailable, window) in cases {
        let cfg = EmuConfig {
            b_power_up: power_up,
            ..config(true, false, TxPolicy::Alternate)
        };
        let (r, _) = run(cfg, 0);
        assert_eq!(r.passed(), unanswered == 0, "{power_up:?}");
        assert_eq!(r.totals.bus_not_fitted, 0);
        assert_eq!(r.totals.unanswered, unanswered, "{power_up:?}");
        let b = &r.link.buses[1];
        assert_eq!(b.unavailable, unavailable, "{power_up:?}");
        assert_eq!(b.power_up_window, window, "{power_up:?}");
        if unanswered > 0 {
            assert!(r.to_string().contains("FAIL: unanswered 155"));
        }
    }
}

#[test]
fn injected_failures_are_all_recovered() {
    if common::capture_missing() {
        return;
    }
    let (r, _) = run(config(true, true, TxPolicy::Alternate), 5);
    assert_eq!(r.responses_lost(), 0);
    assert!(r.failures() > 0);
    assert_eq!(r.link_resends(), r.failures());
    let text = r.to_string();
    assert!(text.contains(&format!(
        "injected transmit failures: {}, resent on the other bus: {}, responses lost: 0",
        r.failures(),
        r.link_resends()
    )));
}

#[test]
fn failing_every_transmission_loses_responses_and_fails() {
    if common::capture_missing() {
        return;
    }
    let (r, _) = run(config(true, true, TxPolicy::Alternate), 1);
    assert!(r.responses_lost() > 0);
    assert!(!r.passed());
    assert!(r.to_string().contains("responses lost"));
}

#[test]
fn the_report_prints_a_row_per_command_and_the_verdict() {
    if common::capture_missing() {
        return;
    }
    let (r, _) = run(config(true, true, TxPolicy::Arrival), 0);
    let text = r.to_string();
    assert!(text.contains("Get Device ID"));
    assert!(text.contains("Reserve Device SDR Repository"));
    assert!(text.trim_end().ends_with("PASS"));
    assert_eq!(Hex(&[0x82, 0x0A]).to_string(), "82 0A");
}

#[test]
fn failing_every_transmission_answers_nothing_and_leaves_every_request_unanswered() {
    if common::capture_missing() {
        return;
    }
    let (r, _) = run(config(true, true, TxPolicy::Alternate), 1);
    assert!(!r.passed());
    assert_eq!(r.answered, 0);
    assert_eq!(r.totals.unanswered, REQUESTS);
    assert_eq!(r.responses_lost(), REQUESTS);
    assert!(r.fail_reasons().iter().any(|x| x == "responses lost 309"));
    assert!(r.to_string().contains("unanswered 309"));
}

#[test]
fn a_resend_on_the_other_bus_is_classified_by_the_bus_that_delivered_it() {
    if common::capture_missing() {
        return;
    }
    let policy = config(true, true, TxPolicy::Arrival);
    let (r, _) = run(policy, 5);
    assert!(r.passed());
    // Arrival policy sends each first attempt on the request's bus, the capture's own bus; every
    // resend (it succeeds, as the next attempt is not a multiple of 5) leaves on the other bus.
    let succeeded_resends = tx_buses(policy, 5)
        .windows(2)
        .filter(|w| !w[0].1 && w[1].1)
        .count() as u32;
    assert!(r.bus_differs > 0);
    assert_eq!(r.bus_differs, succeeded_resends);
    assert_eq!(r.bus_differs, r.link_resends());
}

/// first_request -- index of the first captured request to the IPMC (8-bit address 82h).
fn first_request(vectors: &[Vector]) -> usize {
    vectors
        .iter()
        .position(|v| {
            ipmb::parse(&v.bytes)
                .map(|f| !f.header.is_response() && f.header.dest == 0x82)
                .unwrap_or(false)
        })
        .expect("a request in the capture")
}

#[test]
fn a_request_with_a_bad_checksum_fails_the_run_and_names_its_line() {
    if common::capture_missing() {
        return;
    }
    let mut vectors = load();
    let index = first_request(&vectors);
    let line = vectors[index].line;
    let last = vectors[index].bytes.len() - 1;
    vectors[index].bytes[last] ^= 0xFF;
    let mut emu = Emulator::new(config(true, true, TxPolicy::Arrival)).expect("emulator");
    let r = replay(&mut emu, &vectors, &ReplayOptions::default(), &mut NoTrace);
    assert!(!r.passed());
    assert_eq!(r.malformed, 1);
    assert_eq!(r.malformed_lines, vec![line]);
    assert_eq!(r.requests, REQUESTS - 1);
    assert!(r.fail_reasons().iter().any(|x| x == "malformed frames 1"));
    let text = r.to_string();
    assert!(text.contains(&format!("malformed frames: 1 (vector lines {line})")));
    assert!(text.contains("FAIL: malformed frames 1"));
    // The Link saw the bad frame: its drop counters moved.
    assert_eq!(
        emu.link()
            .counters(vectors[index].bus)
            .rx_errors
            .data_checksum,
        1
    );
}

#[test]
fn an_empty_vector_list_fails_with_its_reason() {
    let vectors = vectors::parse(
        "# nothing
",
    )
    .expect("parse");
    assert!(vectors.is_empty());
    let mut emu = Emulator::new(config(true, true, TxPolicy::Arrival)).expect("emulator");
    let r = replay(&mut emu, &vectors, &ReplayOptions::default(), &mut NoTrace);
    assert!(!r.passed());
    assert_eq!(r.requests, 0);
    assert_eq!(r.fail_reasons(), vec!["no requests addressed to the IPMC"]);
    assert!(r
        .to_string()
        .ends_with("FAIL: no requests addressed to the IPMC"));
}

#[test]
fn the_sampled_state_after_the_capture_is_m4_with_healthy_sensors() {
    if common::capture_missing() {
        return;
    }
    let vectors = load();
    let mut emu = Emulator::new(config(true, true, TxPolicy::Arrival)).expect("emulator");
    let r = replay(&mut emu, &vectors, &ReplayOptions::default(), &mut NoTrace);
    assert!(r.passed());
    let ctx = emu.context();
    assert_eq!(ctx.fru_state.state(), State::M4);
    // Get Sensor Reading bytes (reading, status, state) from the sampled snapshot. Status 40h =
    // scanning enabled (IPMI v2.0 Table 35-14), 20h adds reading unavailable.
    let expected: [[u8; 3]; 7] = [
        // Table 10.2.1-1 byte 4: bit 4 = state M4 (payload power sampled good at start-up).
        [0x00, 0x40, 0x10],
        // Table 10.2.2-1: IPMB-A and IPMB-B Local Control (byte 2 bits 3 and 7), byte 4 bit 3 =
        // both enabled; the capture's Set IPMB State leaves both enabled.
        [0x88, 0x40, 0x08],
        // Table 10.2.3-1 byte 4 bits 1:0 = 01b: functioning properly (sim rails, PGOOD high).
        [0x00, 0x40, 0x01],
        // Table 10.2.4-1 byte 4 bits 1:0 = 01b: every monitored voltage within range (sim rails
        // at their nominal values).
        [0x00, 0x40, 0x01],
        // Table 10.2.5-1 byte 4 = 00h: no threshold flag at the 25 C simulated die temperature.
        [0x00, 0x40, 0x00],
        // Table 10.2.6-1: no payload test has run, reading unavailable (status bit 5).
        [0x00, 0x60, 0x00],
        // Table 10.2.7-1 byte 4 bits 1:0 = 01b: test not in progress.
        [0x00, 0x40, 0x01],
    ];
    for (number, want) in expected.iter().enumerate() {
        let mut out = [0u8; 3];
        let n = ctx.read_sensor(number as u8, &ctx.link.buses, &mut out);
        assert_eq!(n, Ok(3), "sensor {number}");
        // At Tier 2 the event messages of a sensor start enabled (status bit 7). At Tier 3 the
        // capture's Chassis Manager switches them off for every sensor number with Set Sensor
        // Event Enable (04h/28h, flags 00h), and the IPMC now answers that command.
        let mut want = *want;
        if ipmc_defs::IPMC_TIER == 2 {
            want[1] |= 0x80;
        }
        assert_eq!(out, want, "sensor {number}");
    }
}

/// FRU_STATE_SENSOR_TYPE -- sensor type of the FRU State sensor (VITA 46.11 Table 10.2.1-1).
const FRU_STATE_SENSOR_TYPE: u8 = 0xF0;
/// PLATFORM_EVENT -- NetFn 04h command 02h; the request data after the header starts EvMRev,
/// sensor type.
const PLATFORM_EVENT: (u8, u8) = (0x04, 0x02);

/// own_requests -- the requests the IPMC transmitted in a replay (even NetFn), by bus, outcome
/// and bytes.
fn own_requests(config: EmuConfig) -> Vec<(Bus, bool, Vec<u8>)> {
    tx_frames(config, 0)
        .into_iter()
        .filter(|(_, _, bytes)| is_request_frame(bytes))
        .collect()
}

#[test]
fn the_replay_answers_the_fru_state_event_and_the_ipmc_does_not_resend_it() {
    if common::capture_missing() {
        return;
    }
    let policy = config(true, true, TxPolicy::Arrival);
    let (r, _) = run(policy, 0);
    let requests = own_requests(policy);
    if tier1() {
        // A Tier-1 IPMC sends no request (Rule 7-4).
        assert!(requests.is_empty());
        assert_eq!((r.ipmc_requests, r.events), (0, 0));
        assert!(!r.summary_line().contains("ipmc_requests"));
        return;
    }
    println!("{}", r.summary_line());
    // Every request the IPMC sent was counted and answered by the Event Receiver role.
    assert_eq!(r.ipmc_requests as usize, requests.len());
    assert_eq!(r.events, r.ipmc_requests);
    assert!(r.passed());
    let fru_state: Vec<&Vec<u8>> = requests
        .iter()
        .map(|(_, _, bytes)| bytes)
        .filter(|bytes| {
            ipmb::parse(bytes).is_ok_and(|f| {
                (f.header.netfn, f.header.cmd) == PLATFORM_EVENT
                    && f.data.get(1) == Some(&FRU_STATE_SENSOR_TYPE)
            })
        })
        .collect();
    assert!(!fru_state.is_empty(), "the IPMC reports its FRU State");
    // It got 00h at once, so no frame is sent twice (a retry repeats the frame, sequence
    // number included).
    for (i, frame) in requests.iter().map(|r| &r.2).enumerate() {
        assert!(
            !requests.iter().skip(i + 1).any(|r| &r.2 == frame),
            "request {i} was resent: {frame:02X?}"
        );
    }
}

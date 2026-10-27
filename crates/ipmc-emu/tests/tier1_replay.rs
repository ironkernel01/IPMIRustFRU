// tier1_replay.rs - replays vectors/sbc_ab.vec through the Link (IPMB-B fitted and enabled, the
// response policy of the capture: the bus the request arrived on) with all Tier-1 commands
// implemented, and checks every captured request against the captured controller's response:
//
// - the response header matches the captured response's header field for field;
// - the completion code is 00h for every implemented command EXCEPT the documented differences
//   below, which are counted and each tied to its reason;
// - where both controllers answer 00h, the response data length is the captured one;
// - where both answer 00h, the response data is exactly the bytes the specification tables give
//   for this IPMC's state (`check_payload`): the expected bytes are written in this file from
//   IPMI v2.0 Tables 20-2, 34-2, 34-3, 35-2, 35-4 and 35-14 and VITA 46.11 Tables 10.1.3.1-1,
//   10.1.3.9-1, 10.1.3.11-1 and 10.2.1-1 to 10.2.7-1 with the defs.rs values, not by calling
//   the handler code under test. The replay feeds no sensor sample, so FRU State is M0, the
//   snapshot is EMPTY and both buses are in Local Control and enabled.
//
// The captured controller is a 2015-revision SBC IPMC with 40 SDRs; this IPMC has 8 records, its
// own FRU image, and follows the 2022 command scope (decision 7 of PLAN.md section 3.7).

mod common;
use common::load;
use ipmc_core::addr;
use ipmc_core::dispatch::Context;
use ipmc_core::ext::NoExtension;
use ipmc_core::ipmb::{self, NETFN_APP, NETFN_GROUP_EXT, NETFN_SE, NETFN_STORAGE};
use ipmc_core::link::{BusMode, Link, LinkConfig, TxPolicy};
use ipmc_core::trace::NoTrace;
use ipmc_fru::FruStore;
use std::collections::BTreeMap;

const OWN: u8 = 0x82;
const CC_OK: u8 = 0x00;
const CC_INVALID_COMMAND: u8 = 0xC1;
const CC_BAD_RESERVATION: u8 = 0xC5;
const CC_OUT_OF_RANGE: u8 = 0xC9;
const CC_NOT_PRESENT: u8 = 0xCB;
const GET_DEVICE_SDR: u8 = 0x21;
const RESERVE_DEVICE_SDR: u8 = 0x22;

/// Our SDR record lengths, from the ID strings of defs.rs: a compact record is 32 bytes plus
/// the string, the locator record 16 bytes plus its string (IPMI v2.0 Tables 43-2 and 43-8). At
/// Tier-3 the Payload Mode sensor (sensor 7) has a record before the locator.
const RECORD_LENGTHS: [usize; 9] = [
    32 + 9,  // FRU State
    32 + 9,  // IPMB Link
    32 + 10, // FRU Health
    32 + 11, // FRU Voltage
    32 + 15, // FRU Temperature
    32 + 15, // Payload Results
    32 + 14, // Payload Status
    32 + 12, // Payload Mode (Tier-3 only)
    16 + 4,  // IPMC
];

/// record_lengths -- `RECORD_LENGTHS` of the build's Tier: the Payload Mode record only at Tier-3.
fn record_lengths() -> Vec<usize> {
    let (sensors, locator) = RECORD_LENGTHS.split_at(RECORD_LENGTHS.len() - 1);
    let sensors = if ipmc_defs::IPMC_TIER == 3 {
        sensors
    } else {
        &sensors[..sensors.len() - 1]
    };
    [sensors, locator].concat()
}

/// Commands the capture sends that this IPMC answers C1h, with the reason (PLAN.md section 4).
const UNSUPPORTED: [(u8, u8, &str); 6] = [
    (
        NETFN_SE,
        0x00,
        "Set Event Receiver: NA for Tier-1, Rules 4.1.1-16 and 7-4",
    ),
    (
        NETFN_SE,
        0x28,
        "Set Sensor Event Enable: NA for Tier-1, Rule 4.1.1-16",
    ),
    (
        NETFN_GROUP_EXT,
        0x1E,
        "FRU Control Capabilities: optional, owner decision 7",
    ),
    (
        NETFN_GROUP_EXT,
        0x44,
        "Get Mandatory Sensor Numbers: optional, owner decision 7",
    ),
    (
        NETFN_GROUP_EXT,
        0x0C,
        "Set FRU Activation: optional, owner decision 7",
    ),
    (
        NETFN_STORAGE,
        0x49,
        "Set SEL Time: optional for Tier-1, owner decision 7",
    ),
];

/// Commands whose response data length must equal the captured one when both answer 00h.
const SAME_LENGTH: [(u8, u8); 10] = [
    (NETFN_APP, 0x01),       // Get Device ID: 15
    (NETFN_SE, 0x20),        // Get Device SDR Info: 2
    (NETFN_SE, 0x21),        // Get Device SDR: 2 + bytes requested
    (NETFN_SE, 0x22),        // Reserve Device SDR Repository: 2
    (NETFN_SE, 0x2D),        // Get Sensor Reading: 3
    (NETFN_STORAGE, 0x10),   // Get FRU Inventory Area Info: 3
    (NETFN_STORAGE, 0x11),   // Read FRU Data: 1 + 16
    (NETFN_GROUP_EXT, 0x00), // Get VSO Capabilities: VSO byte + 6
    (NETFN_GROUP_EXT, 0x09), // Set IPMB State: VSO byte only
    (NETFN_GROUP_EXT, 0x0D), // Get Device Locator Record ID: VSO byte + 2
];

/// Get Sensor Reading status byte (IPMI v2.0 Table 35-14, byte 3): bit 6 sensor scanning enabled,
/// bit 5 reading/state unavailable; bit 7 (event messages) is 0 because a Tier-1 IPMC sends none.
const STATUS_SCANNING: u8 = 0x40;
const STATUS_UNAVAILABLE: u8 = 0x20;
/// Event messages enabled (status bit 7), set from Tier 2.
const STATUS_EVENTS_ENABLED: u8 = 0x80;
/// The VSO identifier that starts the data of every 2Dh response.
const VSO: u8 = 0x03;

/// expected_device_id -- Get Device ID response data (IPMI v2.0 Table 20-2, bytes 2..16), laid
/// out here from the ipmc-defs values: device ID; revision with the "provides Device SDRs"
/// bit 7; firmware revision 0.1 (major 00h, minor 01h BCD, owner decision 1); IPMI version;
/// additional device support (bit 0 sensor, 1 SDR repository, 2 SEL, 3 FRU inventory, 4 event
/// receiver, 5 event generator, 6 bridge, 7 chassis); manufacturer ID x3 and product ID x2, LS
/// byte first; auxiliary firmware revision x4.
fn expected_device_id() -> [u8; 15] {
    use ipmc_defs as d;
    let byte = |value: u32, index: u32| value.to_le_bytes()[index as usize];
    let revision =
        (u32::from(d::IPMC_DEVICE_PROVIDES_SDRS) << 7) | u32::from(d::IPMC_DEVICE_REVISION);
    let support = u32::from(d::IPMC_SUPPORT_SENSOR)
        | (u32::from(d::IPMC_SUPPORT_SDR_REPOSITORY) << 1)
        | (u32::from(d::IPMC_SUPPORT_SEL) << 2)
        | (u32::from(d::IPMC_SUPPORT_FRU_INVENTORY) << 3)
        | (u32::from(d::IPMC_SUPPORT_EVENT_RECEIVER) << 4)
        | (u32::from(d::IPMC_SUPPORT_EVENT_GENERATOR) << 5)
        | (u32::from(d::IPMC_SUPPORT_BRIDGE) << 6)
        | (u32::from(d::IPMC_SUPPORT_CHASSIS) << 7)
        // From Tier 2 the IPMC generates events (Rule 7-3, bit 5); at Tier 3 it also holds a SEL
        // (Rule 7-8, bit 2).
        | (u32::from(d::IPMC_TIER >= 2) << 5)
        | (u32::from(d::IPMC_TIER >= 3) << 2);
    let aux = d::IPMC_AUX_FIRMWARE.to_le_bytes();
    [
        byte(u32::from(d::IPMC_DEVICE_ID), 0),
        byte(revision, 0),
        0x00,
        0x01,
        byte(u32::from(d::IPMC_IPMI_VERSION), 0),
        byte(support, 0),
        byte(d::IPMC_MANUFACTURER_ID, 0),
        byte(d::IPMC_MANUFACTURER_ID, 1),
        byte(d::IPMC_MANUFACTURER_ID, 2),
        byte(u32::from(d::IPMC_PRODUCT_ID), 0),
        byte(u32::from(d::IPMC_PRODUCT_ID), 1),
        aux[0],
        aux[1],
        aux[2],
        aux[3],
    ]
}

/// expected_sensor_reading -- Get Sensor Reading data (reading, status, state) of sensor
/// `number` in the replay's state, one table per sensor (VITA 46.11 Table 10.2-1 numbering).
fn expected_sensor_reading(number: u8) -> [u8; 3] {
    let mut reading = match number {
        // Table 10.2.1-1 byte 4: bit n = state Mn; the replay never leaves M0.
        0 => [0x00, STATUS_SCANNING, 0x01],
        // Table 10.2.2-1: byte 2 bit 7 IPMB-B and bit 3 IPMB-A in Local Control (override
        // clear), local status 0; byte 4 bit 3 = both IPMBs enabled.
        1 => [0x88, STATUS_SCANNING, 0x08],
        // Tables 10.2.3-1 (Health), 10.2.4-1 (Voltage), 10.2.5-1 (Temperature): no sample was
        // fed, so the reading is unavailable (status bit 5) and bytes 2 and 4 are zero.
        2..=4 => [0x00, STATUS_SCANNING | STATUS_UNAVAILABLE, 0x00],
        // Table 10.2.6-1 Payload Test Results: no payload test has run, reading unavailable.
        5 => [0x00, STATUS_SCANNING | STATUS_UNAVAILABLE, 0x00],
        // Table 10.2.7-1 Payload Test Status: byte 4 bits 1:0 = 01b, test not in progress.
        6 => [0x00, STATUS_SCANNING, 0x01],
        other => panic!("sensor {other} is not a mandatory sensor"),
    };
    // From Tier 2 the event messages of a sensor start enabled (status bit 7, Table 35-14): the
    // IPMC generates events.
    if ipmc_defs::IPMC_TIER >= 2 {
        reading[1] |= STATUS_EVENTS_ENABLED;
    }
    reading
}

/// check_payload -- the exact response data of commands both controllers answer 00h. `request`
/// and `ours` are the data fields of the request and of our response (for a 2Ch/2Dh command
/// including the VSO byte of the response); `captured` is the captured controller's data.
/// Returns true when the command has an exact check here.
fn check_payload(netfn: u8, cmd: u8, request: &[u8], ours: &[u8], captured: &[u8]) -> bool {
    let what = format!("{netfn:02X}h/{cmd:02X}h request {request:02X?}");
    match (netfn, cmd) {
        // Table 20-2.
        (NETFN_APP, 0x01) => assert_eq!(ours, expected_device_id(), "{what}"),
        // Table 35-2: the operation byte selects the count (bit 0: 1b all SDRs, 0b the sensors of
        // the LUN); flags bit 0 = LUN 0 has sensors, bit 7 = 0 static sensor population.
        (NETFN_SE, 0x20) => {
            let records = record_lengths().len();
            let count = if request == [1] { records } else { records - 1 };
            assert_eq!(ours, [u8::try_from(count).unwrap(), 0x01], "{what}");
        }
        // Table 35-4: the reservation IDs run 1, 2, 3 ... like the captured controller's.
        (NETFN_SE, RESERVE_DEVICE_SDR) => assert_eq!(ours, captured, "{what}"),
        // Table 35-14 with VITA 46.11 Tables 10.2.1-1 to 10.2.7-1 (see `expected_sensor_reading`).
        (NETFN_SE, 0x2D) => assert_eq!(ours, expected_sensor_reading(request[0]), "{what}"),
        // Table 34-2: area size LS, MS (IPMC_FRU_SIZE), access by bytes 00h.
        (NETFN_STORAGE, 0x10) => {
            let [lo, hi] = u16::try_from(ipmc_defs::IPMC_FRU_SIZE)
                .unwrap()
                .to_le_bytes();
            assert_eq!(ours, [lo, hi, 0x00], "{what}");
        }
        // Table 34-3: the count returned (1-based) equals the count asked for, then that many
        // bytes of the built FRU image at the requested offset.
        (NETFN_STORAGE, 0x11) => {
            let [_, lo, hi, count] = request else {
                panic!("Read FRU Data has 4 request bytes: {what}");
            };
            let offset = usize::from(u16::from_le_bytes([*lo, *hi]));
            let image = FruStore::new();
            let mut expected = vec![*count];
            expected.extend_from_slice(image.read(offset, usize::from(*count)).unwrap());
            assert_eq!(ours, expected, "{what}");
        }
        // VITA 46.11 Table 10.1.3.1-1: VSO byte; Layer 1 / tier 00h (Tier 1), 01h (Tier 2), 02h
        // (Tier 3: the build's IPMC_TIER less one); interfaces 01b (two,
        // Rule 4.1.1-2) at 100 kHz (400 kHz, 01b in bits 5:4, at Tier-3); VITA 46.11 00h;
        // revision 22h (2022); max FRU Device ID 0; IPMC FRU Device ID 0.
        (NETFN_GROUP_EXT, 0x00) => {
            let tier = u8::try_from(ipmc_defs::IPMC_TIER - 1).expect("tier 1..=3");
            let interfaces = if ipmc_defs::IPMC_TIER >= 3 {
                0x11
            } else {
                0x01
            };
            assert_eq!(
                ours,
                [VSO, tier, interfaces, 0x00, 0x22, 0x00, 0x00],
                "{what}"
            );
        }
        // Set IPMB State (Table 10.1.3.9-1): the response carries only the VSO byte.
        (NETFN_GROUP_EXT, 0x09) => assert_eq!(ours, [VSO], "{what}"),
        // Table 10.1.3.11-1: the type 12h locator record is the last, ID 0007h LS first (0008h
        // at Tier-3, after the Payload Mode record).
        (NETFN_GROUP_EXT, 0x0D) => {
            let id = u8::try_from(record_lengths().len() - 1).unwrap();
            assert_eq!(ours, [VSO, id, 0x00], "{what}");
        }
        _ => return false,
    }
    true
}

/// expected_get_device_sdr -- the completion code this IPMC gives a captured Get Device SDR
/// request, and why it may differ from the captured 00h. `data` is the request data: reservation
/// ID, record ID, offset, count; `reservation` is the latest ID this IPMC handed out.
fn expected_get_device_sdr(data: &[u8], reservation: u16) -> (u8, &'static str) {
    let [r_lo, r_hi, i_lo, i_hi, offset, count] = data else {
        panic!("captured Get Device SDR request has 6 data bytes");
    };
    let (offset, count) = (usize::from(*offset), usize::from(*count));
    if offset != 0 && u16::from_le_bytes([*r_lo, *r_hi]) != reservation {
        return (
            CC_BAD_RESERVATION,
            "stale reservation: the captured ID is not this IPMC's latest",
        );
    }
    let id = usize::from(u16::from_le_bytes([*i_lo, *i_hi]));
    let lengths = record_lengths();
    let Some(length) = lengths.get(id) else {
        return (
            CC_NOT_PRESENT,
            "record ID beyond this IPMC's records (the capture lists 40)",
        );
    };
    if offset > *length || (count != 0xFF && offset + count > *length) {
        return (
            CC_OUT_OF_RANGE,
            "chunk beyond this IPMC's record length (the records differ in length)",
        );
    }
    (CC_OK, "in range")
}

#[test]
fn every_captured_request_gets_the_documented_answer() {
    if common::capture_missing() {
        return;
    }
    let resolved = addr::resolve(0x7F);
    let mut link = Link::new(LinkConfig::bench(TxPolicy::Arrival, false), &resolved);
    link.set_ipmb_state(None, Some(BusMode::LocalControl))
        .expect("enable B");
    let mut ctx = Context::new(resolved);
    let vectors = load();
    let mut reservation = 0u16;
    let mut outcomes: BTreeMap<String, u32> = BTreeMap::new();
    let mut answered = 0;
    let mut payload_checks: BTreeMap<(u8, u8), u32> = BTreeMap::new();
    for pair in vectors.windows(2) {
        let (Ok(request), Ok(captured)) =
            (ipmb::parse(&pair[0].bytes), ipmb::parse(&pair[1].bytes))
        else {
            continue;
        };
        if request.header.is_response()
            || request.header.dest != OWN
            || !captured.header.is_response()
        {
            continue;
        }
        // Feed the request at its capture time and take our response.
        link.on_frame(pair[0].bus, &pair[0].bytes, pair[0].time_ms)
            .expect("accepted");
        link.poll(&mut ctx, &mut NoExtension, pair[0].time_ms, &mut NoTrace);
        let (tx_bus, bytes) = link
            .next_tx()
            .map(|(b, f)| (b, f.to_vec()))
            .expect("every request is answered");
        link.on_tx_result(tx_bus, true, pair[0].time_ms, &mut NoTrace)
            .unwrap();
        let ours = ipmb::parse(&bytes).expect("our response parses");
        assert_eq!(tx_bus, pair[0].bus, "the arrival-bus policy");
        // Every response header matches the captured one.
        assert_eq!(
            ours.header, captured.header,
            "header of {:?}",
            request.header
        );
        let (netfn, cmd) = (request.header.netfn, request.header.cmd);
        let cc = ours.completion_code.expect("completion code");

        // The documented expectation for this command.
        let (expected_cc, why) = if let Some((_, _, reason)) = UNSUPPORTED
            .iter()
            .find(|(n, c, _)| (*n, *c) == (netfn, cmd))
        {
            (CC_INVALID_COMMAND, *reason)
        } else if (netfn, cmd) == (NETFN_SE, GET_DEVICE_SDR) {
            expected_get_device_sdr(request.data, reservation)
        } else {
            (CC_OK, "implemented, in range")
        };
        assert_eq!(
            cc, expected_cc,
            "{netfn:02X}h/{cmd:02X}h data {:02X?}: {why}",
            request.data
        );
        if (netfn, cmd) == (NETFN_SE, RESERVE_DEVICE_SDR) {
            reservation = u16::from_le_bytes([ours.data[0], ours.data[1]]);
        }
        // Where both answer 00h the response structure (data length) is the captured one.
        if cc == CC_OK
            && captured.completion_code == Some(CC_OK)
            && SAME_LENGTH.contains(&(netfn, cmd))
        {
            assert_eq!(
                ours.data.len(),
                captured.data.len(),
                "data length of {netfn:02X}h/{cmd:02X}h"
            );
        }
        // Where both answer 00h the response data is exactly the specified bytes.
        if cc == CC_OK
            && captured.completion_code == Some(CC_OK)
            && check_payload(netfn, cmd, request.data, ours.data, captured.data)
        {
            *payload_checks.entry((netfn, cmd)).or_default() += 1;
        }
        // A 2Dh response starts with the VSO identifier even in an error.
        if netfn == NETFN_GROUP_EXT {
            assert_eq!(ours.data.first(), Some(&0x03));
        }
        *outcomes
            .entry(format!("{netfn:02X}h/{cmd:02X}h -> {cc:02X}h ({why})"))
            .or_default() += 1;
        answered += 1;
    }
    for (what, n) in &outcomes {
        println!("{n:4} x {what}");
    }
    assert_eq!(answered, 309, "requests in the capture");
    // The expected differences of the Get Device SDR requests, counted: this IPMC's reservation
    // IDs run 1, 2, 3 ... exactly as the captured controller's did, so no request meets a stale
    // reservation; 10 chunks fall outside this IPMC's record lengths and 124 name record IDs it
    // does not have. With the Payload Mode record at Tier-3 the capture's reads of record ID 8
    // find a record: 19 answered, 11 out of range, 120 absent.
    let count_of = |prefix: &str| -> u32 {
        outcomes
            .iter()
            .filter(|(what, _)| what.starts_with(prefix))
            .map(|(_, n)| *n)
            .sum()
    };
    let (answered_sdr, out_of_range, absent) = if ipmc_defs::IPMC_TIER == 3 {
        (19, 11, 120)
    } else {
        (16, 10, 124)
    };
    assert_eq!(count_of("04h/21h -> 00h"), answered_sdr);
    assert_eq!(count_of("04h/21h -> C5h"), 0);
    assert_eq!(count_of("04h/21h -> C9h"), out_of_range);
    assert_eq!(count_of("04h/21h -> CBh"), absent);
    // The commands answered C1h: 4 + 78 NA requests and 1 + 3 + 1 + 2 optional ones.
    assert_eq!(count_of("04h/00h -> C1h") + count_of("04h/28h -> C1h"), 82);
    assert_eq!(
        count_of("0Ah/49h -> C1h")
            + count_of("2Ch/0Ch -> C1h")
            + count_of("2Ch/1Eh -> C1h")
            + count_of("2Ch/44h -> C1h"),
        7
    );
    // Exact payload checks that ran, per command: Get Device ID 1, Get Device SDR Info 2,
    // Reserve 40, Get Sensor Reading 8 (every sensor 0..=6 at least once), Get FRU Inventory Area
    // Info 1, Read FRU Data 15, Get VSO Capabilities 1, Set IPMB State 1, Get Device Locator
    // Record ID 1.
    let expected_checks: BTreeMap<(u8, u8), u32> = [
        ((NETFN_APP, 0x01), 1),
        ((NETFN_SE, 0x20), 2),
        ((NETFN_SE, RESERVE_DEVICE_SDR), 40),
        ((NETFN_SE, 0x2D), 8),
        ((NETFN_STORAGE, 0x10), 1),
        ((NETFN_STORAGE, 0x11), 15),
        ((NETFN_GROUP_EXT, 0x00), 1),
        ((NETFN_GROUP_EXT, 0x09), 1),
        ((NETFN_GROUP_EXT, 0x0D), 1),
    ]
    .into_iter()
    .collect();
    assert_eq!(payload_checks, expected_checks);
    assert_eq!(link.dispatch_errors(), 0);
    assert_eq!(link.egress_dropped(), 0);
    assert_eq!(link.late_responses(), 0);
}

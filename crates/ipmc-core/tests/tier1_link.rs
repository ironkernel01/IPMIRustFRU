// tier1_link.rs - Phase 4 behaviour that crosses the link layer: the Set IPMB State hand-off (the
// handler validates, the Link applies right after it returns and before the response is queued)
// and multi-message SDR reads whose chunks arrive on alternating buses.

mod common;
use common::{by_tier, sensor0, Answer, Rig};
use ipmc_core::addr;
use ipmc_core::ipmb::{NETFN_GROUP_EXT, NETFN_SE};
use ipmc_core::link::{BusMode, Link, LinkConfig, TxPolicy};
use ipmc_core::queue::Bus;

const SET_IPMB_STATE: u8 = 0x09;
const RESERVE_DEVICE_SDR: u8 = 0x22;
const GET_DEVICE_SDR: u8 = 0x21;
const CC_OK: u8 = 0x00;
const CC_BAD_FIELD: u8 = 0xCC;
const CC_BAD_RESERVATION: u8 = 0xC5;

/// set_ipmb_state -- sends Set IPMB State for A and B (request bytes after the VSO identifier).
fn set_ipmb_state(rig: &mut Rig, bus: Bus, a: u8, b: u8) -> Answer {
    rig.send(bus, NETFN_GROUP_EXT, SET_IPMB_STATE, &[0x03, a, b])
}

#[test]
fn set_ipmb_state_takes_effect_before_the_response_leaves() {
    let mut rig = Rig::new();
    // Power-up: A enabled, B isolated (Rule 4.1.1-4).
    assert!(rig.link.bus_state(Bus::A).enabled());
    assert!(!rig.link.bus_state(Bus::B).enabled());
    // The Chassis Manager isolates A and puts B in Local Control. The request arrives on A; the
    // change is applied right after the handler returns, so the response cannot use A any more.
    let answer = set_ipmb_state(&mut rig, Bus::A, 0x00, 0x01);
    assert_eq!((answer.cc, answer.data.len()), (CC_OK, 0));
    assert_eq!(rig.link.bus_state(Bus::A).mode, BusMode::Isolated);
    assert!(!rig.link.bus_state(Bus::A).enabled());
    assert!(rig.link.bus_state(Bus::B).enabled());
    assert_eq!(
        answer.bus,
        Bus::B,
        "the response uses the bus the command left enabled"
    );
    assert!(rig.ctx.pending_ipmb.is_none(), "the hand-off is consumed");
    // IPMB-A now drops frames: the Link is isolated from it.
    let frame = rig.frame(NETFN_SE, 0x2D, &[0]);
    assert!(rig.link.on_frame(Bus::A, &frame, rig.now_ms).is_err());
}

#[test]
fn set_ipmb_state_enables_b_like_the_captured_chassis_manager() {
    let mut rig = Rig::new();
    // The capture's request: 03 01 01, IPMB-A and IPMB-B Local Control.
    let answer = set_ipmb_state(&mut rig, Bus::A, 0x01, 0x01);
    assert_eq!((answer.cc, answer.data.len()), (CC_OK, 0));
    assert!(rig.link.bus_state(Bus::A).enabled());
    assert!(rig.link.bus_state(Bus::B).enabled());
    // The IPMB Link sensor now reads both enabled and in local control: 88h, status, 08h; the
    // status byte has bit 7 (events enabled) set at Tier-2/3.
    let reading = rig.send(Bus::B, NETFN_SE, 0x2D, &[1]);
    let status = by_tier([0x40, 0xC0, 0xC0]);
    assert_eq!(
        (reading.cc, reading.data),
        (CC_OK, vec![0x88, status, 0x08])
    );
}

#[test]
fn a_refused_set_ipmb_state_leaves_the_buses_alone() {
    let mut rig = Rig::new();
    // Isolating the only enabled bus would isolate the IPMC (Rule 4.1.8-3).
    let answer = set_ipmb_state(&mut rig, Bus::A, 0x00, 0xFF);
    assert_eq!(answer.cc, CC_BAD_FIELD);
    assert!(rig.link.bus_state(Bus::A).enabled());
    assert!(!rig.link.bus_state(Bus::B).enabled());
    // An unsupported parameter: no change either. 400 kHz (speed 01b) below Tier-3, the
    // reserved speed 10b at Tier-3, which supports FAST-MODE.
    let unsupported_speed = by_tier([0x01, 0x01, 0x02]);
    let answer = rig.send(
        Bus::A,
        NETFN_GROUP_EXT,
        SET_IPMB_STATE,
        &[0x03, 0x01, 0x01, unsupported_speed],
    );
    assert_eq!(answer.cc, CC_BAD_FIELD);
    assert!(!rig.link.bus_state(Bus::B).enabled());
    // A bus not built in cannot be put in Local Control.
    let mut bench = Rig::new();
    bench.link = Link::new(
        LinkConfig {
            b_fitted: false,
            tx_policy: TxPolicy::Alternate,
            requests_allowed: false,
        },
        &addr::resolve(0x7F),
    );
    let answer = set_ipmb_state(&mut bench, Bus::A, 0xFF, 0x01);
    assert_eq!(answer.cc, CC_BAD_FIELD);
}

#[test]
fn a_retried_set_ipmb_state_is_replayed_and_not_applied_twice() {
    let mut rig = Rig::new();
    let frame = rig.frame(NETFN_GROUP_EXT, SET_IPMB_STATE, &[0x03, 0x01, 0x01]);
    let first = rig.send_frame(Bus::A, &frame);
    assert_eq!(first.cc, CC_OK);
    // The Chassis Manager retries the same request on the other bus: answered from the cache.
    let retry = rig.send_frame(Bus::B, &frame);
    assert_eq!((retry.cc, retry.data), (CC_OK, first.data));
    assert_eq!(rig.link.counters(Bus::B).replays, 1);
}

fn sdr_request(reservation: [u8; 2], id: u16, offset: u8, count: u8) -> Vec<u8> {
    let [id_lo, id_hi] = id.to_le_bytes();
    vec![reservation[0], reservation[1], id_lo, id_hi, offset, count]
}

#[test]
fn an_sdr_record_reads_back_across_both_buses_with_one_reservation() {
    let mut rig = Rig::new().both_buses();
    // Reserve on A, then the three chunks on B, A, B: the reservation is link-wide, not per bus.
    let reserved = rig.send(Bus::A, NETFN_SE, RESERVE_DEVICE_SDR, &[]);
    assert_eq!(
        (reserved.cc, &reserved.data[..]),
        (CC_OK, &[0x01, 0x00][..])
    );
    let res = [reserved.data[0], reserved.data[1]];
    let mut record = Vec::new();
    for (bus, offset, count) in [(Bus::B, 0u8, 16u8), (Bus::A, 16, 16), (Bus::B, 32, 9)] {
        let answer = rig.send(
            bus,
            NETFN_SE,
            GET_DEVICE_SDR,
            &sdr_request(res, 0, offset, count),
        );
        assert_eq!(answer.cc, CC_OK, "offset {offset}");
        assert_eq!(&answer.data[..2], &[0x01, 0x00]);
        record.extend_from_slice(&answer.data[2..]);
    }
    assert_eq!(record, sensor0());
    // A second Reserve, from the other bus, makes the first reservation stale (C5h) on both.
    let second = rig.send(Bus::B, NETFN_SE, RESERVE_DEVICE_SDR, &[]);
    assert_eq!(second.data, vec![0x02, 0x00]);
    for bus in [Bus::A, Bus::B] {
        let stale = rig.send(bus, NETFN_SE, GET_DEVICE_SDR, &sdr_request(res, 0, 16, 16));
        assert_eq!(stale.cc, CC_BAD_RESERVATION);
    }
}

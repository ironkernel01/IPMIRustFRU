// trace_vectors.rs - replays the captured IPMB traffic (vectors/sbc_ab.vec) through the codec and dispatcher.

use ipmc_core::addr;
use ipmc_core::commands;
use ipmc_core::dispatch::{cc, dispatch, Context};
use ipmc_core::ipmb::{self, Frame, IPMB_MAX_LEN, NETFN_GROUP_EXT};
use ipmc_core::queue::{Bus, EgressQueue};
use ipmc_core::trace::NoTrace;
mod common;
use common::load;

const MIN_REQUESTS: usize = 300;
/// Responses whose request is missing from the capture.
const MAX_ORPHAN_RESPONSES: usize = 1;

fn is_response(f: &Frame) -> bool {
    f.completion_code.is_some()
}

#[test]
fn every_frame_parses_and_round_trips() {
    for (i, v) in load().iter().enumerate() {
        let f = ipmb::parse(&v.bytes).unwrap_or_else(|e| panic!("frame {i}: {}", e.name()));
        let mut out = [0u8; IPMB_MAX_LEN];
        let n = match f.completion_code {
            Some(code) => ipmb::encode_response(&f.header, code, f.data, &mut out),
            None => ipmb::encode_request(&f.header, f.data, &mut out),
        }
        .unwrap_or_else(|e| panic!("frame {i}: encode {}", e.name()));
        assert_eq!(&out[..n], &v.bytes[..], "frame {i}");
    }
}

#[test]
fn requests_and_responses_pair_up_on_the_same_bus() {
    let vectors = load();
    let (mut requests, mut responses, mut bus_changes, mut orphan_responses) = (0, 0, 0, 0);
    let mut prev_request: Option<(Bus, Frame)> = None;
    let mut last_request_bus: Option<Bus> = None;
    for v in &vectors {
        let f = ipmb::parse(&v.bytes).expect("parse");
        if is_response(&f) {
            responses += 1;
            let Some((bus, req)) = prev_request.take() else {
                // The capture lost one request on the wire (a decoder glitch at t = 25.59 s on
                // IPMB-A, no START row); its response is intact and stays in the vector file.
                orphan_responses += 1;
                continue;
            };
            let (rsp, req) = (f.header, req.header);
            assert_eq!(bus, v.bus, "response on another bus");
            assert_eq!(rsp.seq, req.seq);
            assert_eq!(rsp.cmd, req.cmd);
            assert_eq!(rsp.netfn, req.netfn + 1);
            assert_eq!(rsp.dest, req.src);
            assert_eq!(rsp.src, req.dest);
            assert_eq!(rsp.dest_lun, req.src_lun);
            assert_eq!(rsp.src_lun, req.dest_lun);
        } else {
            requests += 1;
            if last_request_bus.is_some_and(|b| b != v.bus) {
                bus_changes += 1;
            }
            last_request_bus = Some(v.bus);
            prev_request = Some((v.bus, f));
        }
    }
    assert!(
        orphan_responses <= MAX_ORPHAN_RESPONSES,
        "orphans {orphan_responses}"
    );
    assert_eq!(requests, responses - orphan_responses);
    assert!(responses >= MIN_REQUESTS, "responses {responses}");
    assert!(requests >= MIN_REQUESTS, "requests {requests}");
    assert!(bus_changes >= MIN_REQUESTS, "bus changes {bus_changes}");
}

/// Get Device ID data per owner decision 1, typed from IPMI v2.0 Table 20-2: device ID 01h,
/// revision 80h (provides Device SDRs), firmware 0 / 01h BCD, IPMI 02h, support 09h (FRU
/// inventory + sensor device), manufacturer 000000h, product 494Bh little-endian, aux 0.
const DEVICE_ID_DATA: [u8; 15] = [
    0x01, 0x80, 0x00, 0x01, 0x02, 0x09, 0x00, 0x00, 0x00, 0x4B, 0x49, 0x00, 0x00, 0x00, 0x00,
];

/// The oracle for the response addressing is the captured controller's own response to the
/// same request: our response header must match it field for field. Every request is
/// dispatched onto the egress queue (drained as it goes) on the bus it arrived on.
#[test]
fn dispatcher_answers_every_captured_request() {
    let mut ctx = Context::new(addr::resolve(0x7F));
    let mut queue = EgressQueue::new();
    let vectors = load();
    let (mut count, mut device_ids) = (0, 0);
    for pair in vectors.windows(2) {
        let req = ipmb::parse(&pair[0].bytes).expect("parse");
        let captured = ipmb::parse(&pair[1].bytes).expect("parse");
        if is_response(&req) || !is_response(&captured) {
            continue;
        }
        let bus = pair[0].bus;
        dispatch(
            &mut ctx,
            &req,
            bus,
            pair[0].time_ms,
            &mut queue,
            &mut NoTrace,
        )
        .expect("dispatch");
        let frame = queue.pop().expect("response queued");
        assert!(queue.is_empty());
        assert_eq!(frame.bus, bus);
        let resp = ipmb::parse(frame.as_bytes()).expect("response re-parses");
        assert_eq!(resp.header, captured.header);
        match commands::find(req.header.netfn, req.header.cmd) {
            None => {
                assert_eq!(resp.completion_code, Some(cc::INVALID_CMD));
                // A 2Dh response carries the VSO identifier first, even in an error.
                let expected: &[u8] = if req.header.netfn == NETFN_GROUP_EXT {
                    &[0x03]
                } else {
                    &[]
                };
                assert_eq!(resp.data, expected);
            }
            Some(row) if (row.netfn, row.cmd) == (0x06, 0x01) => {
                assert_eq!(resp.completion_code, Some(cc::OK));
                assert_eq!(resp.data, &DEVICE_ID_DATA[..]);
                device_ids += 1;
            }
            // Completion codes of the implemented commands are checked in tier1_replay.rs, which
            // lists the documented differences from the captured controller.
            Some(_) => assert!(resp.completion_code.is_some()),
        }
        count += 1;
    }
    assert!(count >= MIN_REQUESTS, "requests answered {count}");
    assert!(device_ids >= 1, "no Get Device ID request in the capture");
    assert_eq!(queue.dropped(), 0);
}

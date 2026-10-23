// common/mod.rs - helpers shared by the ipmc-core integration tests (direct dispatch calls and the
// one-request-at-a-time link rig).

use ipmc_core::addr;
use ipmc_core::dispatch::{dispatch, Context};
use ipmc_core::ipmb::{self, Header, NETFN_GROUP_EXT};
use ipmc_core::link::{BusMode, Link, LinkConfig, TxPolicy};
use ipmc_core::queue::{Bus, EgressQueue};
use ipmc_core::trace::NoTrace;

/// The requester of every call: the Chassis Manager at IPMB 20h; the responder is GA 1, 82h.
#[allow(dead_code)]
pub const CM: u8 = 0x20;
#[allow(dead_code)]
pub const OWN: u8 = 0x82;
/// Sequence number of every direct call; the response frames of the tests are written for it.
#[allow(dead_code)]
pub const SEQ: u8 = 5;

/// Reply -- one dispatched response: completion code, data after it (for a Group Extension reply
/// the data after the VSO byte) and the whole frame.
// Each integration test binary uses a different subset.
#[allow(dead_code)]
pub struct Reply {
    pub cc: u8,
    pub data: Vec<u8>,
    pub frame: Vec<u8>,
}

/// call -- dispatches one request straight to the handlers (no link layer) and returns the reply.
#[allow(dead_code)]
pub fn call(ctx: &mut Context, netfn: u8, cmd: u8, data: &[u8]) -> Reply {
    let header = Header {
        dest: OWN,
        netfn,
        dest_lun: 0,
        src: CM,
        seq: SEQ,
        src_lun: 0,
        cmd,
    };
    let mut buf = [0u8; ipmb::IPMB_MAX_LEN];
    let n = ipmb::encode_request(&header, data, &mut buf).expect("encode request");
    let request = ipmb::parse(&buf[..n]).expect("parse request");
    let mut queue = EgressQueue::new();
    dispatch(ctx, &request, Bus::A, 0, &mut queue, &mut NoTrace).expect("dispatch");
    let frame = queue.pop().expect("a response").as_bytes().to_vec();
    let response = ipmb::parse(&frame).expect("response parses");
    Reply {
        cc: response.completion_code.expect("completion code"),
        data: response.data.to_vec(),
        frame,
    }
}

/// call_vso -- `call` for a Group Extension command: the VSO identifier 03h goes in front of
/// `data` and is checked, then stripped, on the way back.
#[allow(dead_code)]
pub fn call_vso(ctx: &mut Context, cmd: u8, data: &[u8]) -> Reply {
    let mut request = vec![0x03];
    request.extend_from_slice(data);
    let mut reply = call(ctx, ipmb::NETFN_GROUP_EXT, cmd, &request);
    assert_eq!(
        reply.data.first(),
        Some(&0x03),
        "response starts with the VSO byte"
    );
    reply.data.remove(0);
    reply
}

/// STEP_MS -- time between requests of the rig, milliseconds (the capture runs one request per
/// 60 ms).
#[allow(dead_code)]
pub const STEP_MS: u32 = 60;

/// Rig -- a link with IPMB-B fitted and a context, driven one request at a time.
#[allow(dead_code)]
pub struct Rig {
    pub link: Link,
    pub ctx: Context,
    pub seq: u8,
    pub now_ms: u32,
}

/// Answer -- the response of one request: the bus it went out on, its completion code and data
/// (for a Group Extension reply, after the VSO byte).
#[allow(dead_code)]
pub struct Answer {
    pub bus: Bus,
    pub cc: u8,
    pub data: Vec<u8>,
}

#[allow(dead_code)]
impl Rig {
    pub fn new() -> Rig {
        let resolved = addr::resolve(0x7F);
        let config = LinkConfig {
            b_fitted: true,
            tx_policy: TxPolicy::Alternate,
        };
        Rig {
            link: Link::new(config, &resolved),
            ctx: Context::new(resolved),
            seq: 0,
            now_ms: 0,
        }
    }

    /// both_buses -- IPMB-B in Local Control as well (what the Chassis Manager's Set IPMB State does).
    pub fn both_buses(mut self) -> Rig {
        self.link
            .set_ipmb_state(None, Some(BusMode::LocalControl))
            .unwrap();
        self
    }

    pub fn frame(&mut self, netfn: u8, cmd: u8, data: &[u8]) -> Vec<u8> {
        self.seq = (self.seq + 1) & 0x3F;
        let header = Header {
            dest: OWN,
            netfn,
            dest_lun: 0,
            src: CM,
            seq: self.seq,
            src_lun: 0,
            cmd,
        };
        let mut buf = [0u8; ipmb::IPMB_MAX_LEN];
        let n = ipmb::encode_request(&header, data, &mut buf).unwrap();
        buf[..n].to_vec()
    }

    /// send_frame -- hands `frame` to the link as received on `bus`, polls, and collects the one
    /// response, reporting its transmit as successful.
    pub fn send_frame(&mut self, bus: Bus, frame: &[u8]) -> Answer {
        self.now_ms += STEP_MS;
        self.link
            .on_frame(bus, frame, self.now_ms)
            .expect("accepted");
        self.link.poll(&mut self.ctx, self.now_ms, &mut NoTrace);
        let (tx_bus, bytes) = self
            .link
            .next_tx()
            .map(|(b, f)| (b, f.to_vec()))
            .expect("a response");
        self.link
            .on_tx_result(tx_bus, true, self.now_ms, &mut NoTrace)
            .unwrap();
        let response = ipmb::parse(&bytes).unwrap();
        let mut data = response.data.to_vec();
        if response.header.netfn == NETFN_GROUP_EXT | 1 {
            assert_eq!(data.remove(0), 0x03);
        }
        Answer {
            bus: tx_bus,
            cc: response.completion_code.unwrap(),
            data,
        }
    }

    pub fn send(&mut self, bus: Bus, netfn: u8, cmd: u8, data: &[u8]) -> Answer {
        let frame = self.frame(netfn, cmd, data);
        self.send_frame(bus, &frame)
    }
}

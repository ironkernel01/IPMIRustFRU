// common/mod.rs - helpers shared by the ipmc-core integration tests (direct dispatch calls and the
// one-request-at-a-time link rig).

use ipmc_core::addr;
use ipmc_core::dispatch::{dispatch, Context};
use ipmc_core::ext::NoExtension;
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

/// by_tier -- the literal expectation of the build's Tier from a (Tier-1, Tier-2, Tier-3) table.
#[allow(dead_code)]
pub fn by_tier<T: Copy>(table: [T; 3]) -> T {
    table[ipmc_defs::IPMC_TIER as usize - 1]
}

/// Sensor 0's record at Tier-1, hand-written from IPMI v2.0 Table 43-2 (see sdr.rs for the field
/// walk); `sensor0` applies the bytes that differ per Tier.
const SENSOR0_TIER1: [u8; 41] = [
    0x00, 0x00, 0x51, 0x02, 0x24, 0x82, 0x00, 0x00, 0xA0, 0x60, 0x01, 0x43, 0xF0, 0x6F, 0x00, 0x00,
    0x00, 0x00, 0x53, 0x00, 0xC0, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC9,
    b'F', b'R', b'U', b' ', b'S', b't', b'a', b't', b'e',
];

/// sensor0 -- sensor 0's record as the build's Tier writes it: sensor initialization (byte 10),
/// capabilities (byte 11), the assertion event mask (bytes 14..16) and the reading mask (bytes
/// 18..20) differ per Tier (Table 43-2; the Tier-2 masks are VITA Table 10.2.1-1's 0053h, Tier-3
/// adds M5: 0073h).
#[allow(dead_code)]
pub fn sensor0() -> [u8; 41] {
    let (init, caps, assertion, reading) = by_tier([
        (0x01, 0x43, 0x0000u16, 0x0053u16),
        (0x03, 0x42, 0x0053, 0x0053),
        (0x03, 0x40, 0x0073, 0x0073),
    ]);
    let mut record = SENSOR0_TIER1;
    record[10] = init;
    record[11] = caps;
    record[14..16].copy_from_slice(&assertion.to_le_bytes());
    record[18..20].copy_from_slice(&reading.to_le_bytes());
    record
}

/// SENSOR_COUNT -- the sensors the build's Tier has: sensors 0..=6, and sensor 7 (Payload Mode)
/// at Tier-3. The locator record is the record after them, so its ID is this number.
#[allow(dead_code)]
pub const SENSOR_COUNT: u8 = if ipmc_defs::IPMC_TIER == 3 { 8 } else { 7 };

/// The management controller device locator record at Tier-1, from Table 43-8; `locator`
/// applies the bytes that differ per Tier.
const LOCATOR_TIER1: [u8; 20] = [
    0x07, 0x00, 0x51, 0x12, 0x0F, 0x82, 0x00, 0x01, 0x0B, 0x00, 0x00, 0x00, 0xA0, 0x60, 0x00, 0xC4,
    b'I', b'P', b'M', b'C',
];

/// locator -- the locator record as the build's Tier writes it: its ID (the sensor count), global
/// initialization (byte 7) and capabilities (byte 8).
#[allow(dead_code)]
pub fn locator() -> [u8; 20] {
    let (global_init, caps) = by_tier([(0x01, 0x0B), (0x00, 0x2B), (0x00, 0x2F)]);
    let mut record = LOCATOR_TIER1;
    record[0] = SENSOR_COUNT;
    record[7] = global_init;
    record[8] = caps;
    record
}

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
            requests_allowed: false,
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
        self.link
            .poll(&mut self.ctx, &mut NoExtension, self.now_ms, &mut NoTrace);
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

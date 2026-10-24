//! requester.rs - the IPMB requester: the one request this IPMC sends at a time.
//!
//! A Tier-1 IPMC sends no request (VITA 46.11 Rule 7-4); a Tier-2/3 IPMC is an Event Generator
//! (Rule 7-3) and sends its requests through this module. The extension calls `submit`; the
//! `Link` hands the frame to a bus driver (`Link::next_tx`), reports the outcome and the
//! responses it receives, and steps the timing, so this module holds the request state only and
//! never touches a bus.
//!
//! ```text
//! submit --> Due --next_tx--> Sending --tx ok--> Waiting --response--> Done --take_completion--> Idle
//!             ^                  | tx failed          | T3 timeout
//!             |                  v                    +-- Due (retry), or Done with NoResponse
//!             +-- T6 min -- Backoff (other bus enabled)
//! ```
//!
//! Rules: 4.1.1-9 a request times out after T3max = IPMB_REQ_TIMEOUT_MS; 4.1.1-10 it is retried
//! until a response arrives or C1 = IPMB_REQ_RETRIES retries are used; Observation 4.1.1-6 a retry
//! keeps the sequence number; Table 4.1.1-1 T6 spaces the retries (a failed transmission waits
//! `T6_MIN_MS` before it goes out on the other bus), T4 spaces new requests (IPMB_REQ_GAP_MS,
//! counted from the end of the previous request's first transmission) and T9 holds a sequence
//! number out of use for `SEQ_EXPIRY_MS` after its latest transmission.

use crate::ipmb::{self, Frame, Header, IPMB_MAX_LEN, IPMB_RESPONSE_DATA_MAX};
use crate::queue::Bus;

/// SEQ_EXPIRY_MS -- T9, the time before a sequence value may be used again (VITA 46.11 Table
/// 4.1.1-1: 5 s). A spec value, not a tunable.
pub const SEQ_EXPIRY_MS: u32 = 5000;
/// T6_MIN_MS -- the least time between request retries (VITA 46.11 Table 4.1.1-1 T6: 60 ms
/// minimum, 250 ms maximum). A failed transmission waits this long before the retry goes out on
/// the other bus. A spec value, not a tunable.
pub const T6_MIN_MS: u32 = 60;
/// SEQ_COUNT -- the rqSeq field is 6 bits (IPMI v2.0 section 13.8).
const SEQ_COUNT: usize = 64;
/// SEQ_MASK -- the 6 bits of rqSeq.
const SEQ_MASK: u8 = 0x3F;
/// TIMEOUT_MS -- wait for a response before a retry (ipmi_defs.h IPMB_REQ_TIMEOUT_MS).
pub const TIMEOUT_MS: u32 = ipmc_defs::IPMB_REQ_TIMEOUT_MS;
/// RETRIES -- retransmissions after the first one (ipmi_defs.h IPMB_REQ_RETRIES, guarded 3..=10;
/// 10 is C1max, Rule 4.1.1-10).
pub const RETRIES: u8 = ipmc_defs::IPMB_REQ_RETRIES as u8;
/// GAP_MS -- least time between two new requests (ipmi_defs.h IPMB_REQ_GAP_MS).
const GAP_MS: u32 = ipmc_defs::IPMB_REQ_GAP_MS;
/// MAX_TRANSMISSIONS -- the first transmission and every retry.
pub const MAX_TRANSMISSIONS: u8 = 1 + RETRIES;

/// Request -- what a caller asks the requester to send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request<'a> {
    /// Responder IPMB address, 8-bit form (the Chassis Manager is 20h).
    pub dest_addr8: u8,
    /// Responder LUN (rsLUN).
    pub dest_lun: u8,
    /// Request NetFn (even).
    pub netfn: u8,
    /// Command code.
    pub cmd: u8,
    /// Request data.
    pub data: &'a [u8],
}

/// RequestError -- why `submit` refused. Nothing is sent and no state changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// This build sends no requests (Tier 1, Rule 7-4).
    NotAllowed,
    /// A request is outstanding, or its completion has not been taken.
    Busy,
    /// Less than IPMB_REQ_GAP_MS since the previous submit (T4).
    TooSoon,
    /// Every one of the 64 sequence numbers was transmitted less than `SEQ_EXPIRY_MS` ago (T9).
    SeqNotExpired,
    /// The frame cannot be built: it does not fit IPMB_MAX_LEN, or a header field is out of
    /// range (odd NetFn, an address with bit 0 set, a LUN above 3).
    TooLong,
}

impl RequestError {
    /// name -- log name of the error.
    pub const fn name(self) -> &'static str {
        match self {
            RequestError::NotAllowed => "NotAllowed",
            RequestError::Busy => "Busy",
            RequestError::TooSoon => "TooSoon",
            RequestError::SeqNotExpired => "SeqNotExpired",
            RequestError::TooLong => "TooLong",
        }
    }
}

/// Answer -- the response to a request: completion code and the data after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answer {
    /// Completion code of the response.
    pub completion_code: u8,
    len: usize,
    bytes: [u8; IPMB_RESPONSE_DATA_MAX],
}

impl Answer {
    /// data -- the response data after the completion code.
    pub fn data(&self) -> &[u8] {
        self.bytes.get(..self.len).unwrap_or(&[])
    }
}

/// NoResponse -- the request got no matching response after 1 + IPMB_REQ_RETRIES transmissions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoResponse;

/// Completion -- how a request ended: the sequence number it went out with and the answer, or
/// NoResponse after 1 + IPMB_REQ_RETRIES transmissions with no matching response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    /// Sequence number of the request.
    pub seq: u8,
    /// Request NetFn.
    pub netfn: u8,
    /// Command code.
    pub cmd: u8,
    /// The response, or NoResponse.
    pub outcome: Result<Answer, NoResponse>,
}

/// Phase -- where the outstanding request stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// No request.
    Idle,
    /// A transmission is due; the link hands the frame to a driver.
    Due,
    /// Handed to a driver on this bus; the outcome is not known yet.
    Sending(Bus),
    /// The transmission failed with the other bus enabled; the retry is due `T6_MIN_MS` after
    /// this time.
    Backoff(u32),
    /// Transmitted; the T3 wait started at this time.
    Waiting(u32),
    /// Cancelled while handed to a driver on this bus: the transmission finishes, nothing follows.
    Cancelling(Bus),
    /// Ended; the completion waits for `take_completion`.
    Done,
}

/// TxReport -- what `tx_done` tells the link for a trace line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TxReport {
    pub seq: u8,
    pub netfn: u8,
    pub cmd: u8,
    /// Transmissions so far, this one included (1 = the first).
    pub transmissions: u8,
}

/// Ended -- what the link traces once when a request ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ended {
    pub seq: u8,
    pub netfn: u8,
    pub cmd: u8,
    /// Completion code of the answer, None for NoResponse.
    pub completion_code: Option<u8>,
    pub transmissions: u8,
}

/// Requester -- one outstanding request, its frame for the retries and its timing.
pub struct Requester {
    allowed: bool,
    own_addr8: u8,
    phase: Phase,
    frame: [u8; IPMB_MAX_LEN],
    frame_len: usize,
    /// Responder address and LUN, NetFn and command of the request, for the match.
    dest_addr8: u8,
    dest_lun: u8,
    netfn: u8,
    cmd: u8,
    seq: u8,
    /// Transmissions handed to a driver for this request.
    transmissions: u8,
    /// Bus of the latest transmission of this request (set by `start_tx`, cleared by `submit`):
    /// the retry prefers the other one (Rules 4.1.1-6 and 4.1.1-7), whatever else the link sent
    /// meanwhile.
    last_bus: Option<Bus>,
    /// rqSeq the next accepted submit uses.
    next_seq: u8,
    /// Time of the latest transmission with each rqSeq value (T9); valid where `seq_used` has the
    /// bit.
    seq_used_ms: [u32; SEQ_COUNT],
    /// Bit n set: rqSeq n has been transmitted (one bit per `SEQ_COUNT` value).
    seq_used: u64,
    /// Time the first transmission of the latest request ended (T4), None before the first.
    last_first_tx_ms: Option<u32>,
    completion: Option<Completion>,
    /// The completion has not been traced yet.
    untraced: bool,
}

impl Requester {
    /// new -- an idle requester; `allowed` false (Tier 1, Rule 7-4) refuses every submit.
    /// `first_seq` is the rqSeq of the first request (6 bits): a Cold Reset keeps the sequence
    /// counting across the restart, so the next request cannot repeat a value the Event Receiver
    /// saw inside T9. The own address starts at 0; `with_own_address` sets the rqSA the
    /// requests carry.
    pub const fn new(allowed: bool, first_seq: u8) -> Requester {
        Requester {
            allowed,
            own_addr8: 0,
            phase: Phase::Idle,
            frame: [0; IPMB_MAX_LEN],
            frame_len: 0,
            dest_addr8: 0,
            dest_lun: 0,
            netfn: 0,
            cmd: 0,
            seq: 0,
            transmissions: 0,
            last_bus: None,
            next_seq: first_seq & SEQ_MASK,
            seq_used_ms: [0; SEQ_COUNT],
            seq_used: 0,
            last_first_tx_ms: None,
            completion: None,
            untraced: false,
        }
    }

    /// with_own_address -- sets the 8-bit IPMB address the requests carry as rqSA and the
    /// responses must be addressed to.
    pub const fn with_own_address(mut self, own_addr8: u8) -> Requester {
        self.own_addr8 = own_addr8;
        self
    }

    /// with_recent_seqs -- books every rqSeq whose bit is set in `recent` as transmitted at
    /// `now_ms` (the start time of this requester), so each stays out of use for a full T9 from
    /// the start. A restart hands over `recent_seqs` of the requester it replaces: the Event
    /// Receiver saw those values inside T9 before the restart, and the real age of each is at
    /// least 0, so counting from the restart is the conservative choice (VITA 46.11 Table
    /// 4.1.1-1 T9).
    pub const fn with_recent_seqs(mut self, recent: u64, now_ms: u32) -> Requester {
        let mut index = 0;
        while index < SEQ_COUNT {
            if (recent >> index) & 1 == 1 {
                self.seq_used_ms[index] = now_ms;
            }
            index += 1;
        }
        self.seq_used |= recent;
        self
    }

    /// recent_seqs -- bit n set when rqSeq n was transmitted less than `SEQ_EXPIRY_MS` before
    /// `now_ms` (T9): the input of `with_recent_seqs` for the requester after a restart.
    pub fn recent_seqs(&self, now_ms: u32) -> u64 {
        (0..SEQ_COUNT).fold(0, |mask, index| {
            let recent = u8::try_from(index)
                .ok()
                .and_then(|seq| self.seq_used_at(seq))
                .is_some_and(|t| now_ms.wrapping_sub(t) < SEQ_EXPIRY_MS);
            mask | (u64::from(recent) << index)
        })
    }

    /// seq_blocked -- `seq` was transmitted less than `SEQ_EXPIRY_MS` before `now_ms` (T9).
    fn seq_blocked(&self, seq: u8, now_ms: u32) -> bool {
        self.seq_used_at(seq)
            .is_some_and(|t| now_ms.wrapping_sub(t) < SEQ_EXPIRY_MS)
    }

    /// submit -- builds the request frame (rqSA = own address, rqLUN 0, next rqSeq) and queues it
    /// for transmission; returns the sequence number. Err: NotAllowed (Tier-1 build), Busy (one
    /// is outstanding or a completion is not yet taken), TooSoon (less than IPMB_REQ_GAP_MS since
    /// the previous submit), SeqNotExpired (all 64 sequence numbers were transmitted less than
    /// T9 = 5 s ago; a blocked `next_seq` is skipped for the next free value, in order, wrapping),
    /// TooLong (the frame cannot be built, see `RequestError::TooLong`). A refused
    /// submit changes nothing and sends nothing.
    pub fn submit(&mut self, req: &Request, now_ms: u32) -> Result<u8, RequestError> {
        if !self.allowed {
            return Err(RequestError::NotAllowed);
        }
        if self.busy() {
            return Err(RequestError::Busy);
        }
        if self
            .last_first_tx_ms
            .is_some_and(|t| now_ms.wrapping_sub(t) < GAP_MS)
        {
            return Err(RequestError::TooSoon);
        }
        let seq = (0..SEQ_COUNT)
            .filter_map(|step| u8::try_from(step).ok())
            .map(|step| self.next_seq.wrapping_add(step) & SEQ_MASK)
            .find(|seq| !self.seq_blocked(*seq, now_ms))
            .ok_or(RequestError::SeqNotExpired)?;
        let header = Header {
            dest: req.dest_addr8,
            netfn: req.netfn,
            dest_lun: req.dest_lun,
            src: self.own_addr8,
            seq,
            src_lun: 0,
            cmd: req.cmd,
        };
        let len = ipmb::encode_request(&header, req.data, &mut self.frame)
            .map_err(|_| RequestError::TooLong)?;
        self.frame_len = len;
        self.dest_addr8 = req.dest_addr8;
        self.dest_lun = req.dest_lun;
        self.netfn = req.netfn;
        self.cmd = req.cmd;
        self.seq = seq;
        self.transmissions = 0;
        self.last_bus = None;
        self.completion = None;
        self.phase = Phase::Due;
        self.next_seq = (seq + 1) & SEQ_MASK;
        Ok(seq)
    }

    /// next_seq -- the rqSeq the next accepted submit uses (kept across a Cold Reset).
    pub const fn next_seq(&self) -> u8 {
        self.next_seq
    }

    /// seq_used_at -- the time of the latest transmission with `seq`, None when it has none.
    fn seq_used_at(&self, seq: u8) -> Option<u32> {
        let index = usize::from(seq);
        let used = (self.seq_used >> index) & 1 == 1;
        self.seq_used_ms.get(index).copied().filter(|_| used)
    }

    /// stamp_transmission -- books a finished transmission of the current request: T9 counts
    /// from its sequence number's latest transmission, T4 from the first transmission of the
    /// request.
    fn stamp_transmission(&mut self, now_ms: u32) {
        if let Some(slot) = self.seq_used_ms.get_mut(usize::from(self.seq)) {
            *slot = now_ms;
            self.seq_used |= 1 << self.seq;
        }
        if self.transmissions == 1 {
            self.last_first_tx_ms = Some(now_ms);
        }
    }

    /// busy -- a request is outstanding, or its completion has not been taken.
    pub fn busy(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// cancel -- drops the outstanding request and any completion not yet taken: no further
    /// transmission and no completion. A frame already handed to a driver finishes its one
    /// transmission (`tx_done` then ends it without a retry); a second `cancel` before that keeps
    /// the request busy, because the driver still owns the frame. The sequence number stays used
    /// for T9. The extension calls it when the request lost its purpose (a new Event Receiver,
    /// IPMI v2.0 29.1: FFh disables Event Message generation entirely).
    pub fn cancel(&mut self) {
        self.completion = None;
        self.untraced = false;
        self.phase = match self.phase {
            Phase::Sending(bus) | Phase::Cancelling(bus) => Phase::Cancelling(bus),
            _ => Phase::Idle,
        };
    }

    /// take_completion -- the finished request's completion, once; the requester is idle again.
    pub fn take_completion(&mut self) -> Option<Completion> {
        if self.phase != Phase::Done {
            return None;
        }
        self.phase = Phase::Idle;
        self.completion.take()
    }

    // ---- the link's side ----

    /// due -- a transmission waits for a driver.
    pub(crate) fn due(&self) -> bool {
        self.phase == Phase::Due
    }

    /// sending_bus -- the bus of the frame handed to a driver whose outcome is open (a cancelled
    /// request included), None otherwise. The one holder of that fact.
    pub(crate) fn sending_bus(&self) -> Option<Bus> {
        match self.phase {
            Phase::Sending(bus) | Phase::Cancelling(bus) => Some(bus),
            _ => None,
        }
    }

    /// last_bus -- the bus of the latest transmission of the current request, None before its
    /// first one.
    pub(crate) fn last_bus(&self) -> Option<Bus> {
        self.last_bus
    }

    /// frame -- the request frame as it goes on the wire.
    pub(crate) fn frame(&self) -> &[u8] {
        self.frame.get(..self.frame_len).unwrap_or(&[])
    }

    /// start_tx -- the link handed the frame to a driver on `bus`: counts the transmission.
    pub(crate) fn start_tx(&mut self, bus: Bus) {
        self.phase = Phase::Sending(bus);
        self.last_bus = Some(bus);
        self.transmissions = self.transmissions.saturating_add(1);
    }

    /// tx_done -- the driver's outcome for the transmission at `now_ms`. Success starts the T3
    /// wait. A failure is retransmitted on the other bus `T6_MIN_MS` later when
    /// `other_bus_enabled` and a transmission is left, else the same bus gets it at the next
    /// timeout (the wait starts now). Returns the data for the trace line; None when the request
    /// ended while the frame was on the wire.
    pub(crate) fn tx_done(
        &mut self,
        ok: bool,
        now_ms: u32,
        other_bus_enabled: bool,
    ) -> Option<TxReport> {
        self.sending_bus()?;
        self.stamp_transmission(now_ms);
        if matches!(self.phase, Phase::Cancelling(_)) {
            self.phase = Phase::Idle;
            return None;
        }
        self.phase = if !ok && other_bus_enabled && self.transmissions < MAX_TRANSMISSIONS {
            Phase::Backoff(now_ms)
        } else {
            Phase::Waiting(now_ms)
        };
        Some(TxReport {
            seq: self.seq,
            netfn: self.netfn,
            cmd: self.cmd,
            transmissions: self.transmissions,
        })
    }

    /// step -- the timing: a T6 wait after a failed transmission that has run out marks the
    /// retransmission due; a T3 wait that has run out ends the request with NoResponse when every
    /// transmission is used, else marks the retransmission due.
    pub(crate) fn step(&mut self, now_ms: u32) {
        match self.phase {
            Phase::Backoff(since) if now_ms.wrapping_sub(since) >= T6_MIN_MS => {
                self.phase = Phase::Due;
            }
            Phase::Waiting(since) if now_ms.wrapping_sub(since) >= TIMEOUT_MS => {
                if self.transmissions >= MAX_TRANSMISSIONS {
                    self.finish(Err(NoResponse));
                } else {
                    self.phase = Phase::Due;
                }
            }
            _ => {}
        }
    }

    /// on_response -- offers a valid response frame; true when it answers the outstanding
    /// request (dest = own address, src = the responder, NetFn + 1, command, sequence number and
    /// both LUNs equal) and ends it. A frame that matches nothing, or whose data exceeds
    /// IPMB_RESPONSE_DATA_MAX, is for the caller to drop (the request then times out).
    pub(crate) fn on_response(&mut self, frame: &Frame) -> bool {
        let outstanding = frame.data.len() <= IPMB_RESPONSE_DATA_MAX
            && matches!(
                self.phase,
                Phase::Due | Phase::Sending(_) | Phase::Backoff(_) | Phase::Waiting(_)
            );
        let h = &frame.header;
        let matches = outstanding
            && h.dest == self.own_addr8
            && h.src == self.dest_addr8
            && h.netfn == self.netfn.wrapping_add(1)
            && h.cmd == self.cmd
            && h.seq == self.seq
            && h.src_lun == self.dest_lun
            && h.dest_lun == 0;
        let Some(code) = frame.completion_code.filter(|_| matches) else {
            return false;
        };
        let mut answer = Answer {
            completion_code: code,
            len: 0,
            bytes: [0; IPMB_RESPONSE_DATA_MAX],
        };
        let Some(dst) = answer.bytes.get_mut(..frame.data.len()) else {
            return false;
        };
        dst.copy_from_slice(frame.data);
        answer.len = frame.data.len();
        self.finish(Ok(answer));
        true
    }

    /// ended -- the end of a request, once, for the link to trace.
    pub(crate) fn ended(&mut self) -> Option<Ended> {
        if !self.untraced {
            return None;
        }
        self.untraced = false;
        let done = self.completion.as_ref()?;
        Some(Ended {
            seq: done.seq,
            netfn: done.netfn,
            cmd: done.cmd,
            completion_code: done.outcome.as_ref().ok().map(|a| a.completion_code),
            transmissions: self.transmissions,
        })
    }

    fn finish(&mut self, outcome: Result<Answer, NoResponse>) {
        self.completion = Some(Completion {
            seq: self.seq,
            netfn: self.netfn,
            cmd: self.cmd,
            outcome,
        });
        self.phase = Phase::Done;
        self.untraced = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: u8 = 0x82;
    const CM: u8 = 0x20;
    const NETFN_SE: u8 = 0x04;
    const PLATFORM_EVENT: u8 = 0x02;
    const EVENT_DATA: [u8; 7] = [0x04, 0xF0, 0x00, 0x6F, 0xA1, 0x00, 0x00];

    fn requester() -> Requester {
        Requester::new(true, 0).with_own_address(OWN)
    }

    fn event() -> Request<'static> {
        Request {
            dest_addr8: CM,
            dest_lun: 0,
            netfn: NETFN_SE,
            cmd: PLATFORM_EVENT,
            data: &EVENT_DATA,
        }
    }

    #[test]
    fn the_frame_of_a_platform_event_is_exact() {
        let mut r = requester();
        assert_eq!(r.submit(&event(), 0), Ok(0));
        assert_eq!(
            r.frame(),
            [0x20, 0x10, 0xD0, 0x82, 0x00, 0x02, 0x04, 0xF0, 0x00, 0x6F, 0xA1, 0x00, 0x00, 0x78]
        );
    }

    #[test]
    fn submit_refusals_leave_the_state_alone() {
        let mut off = Requester::new(false, 0).with_own_address(OWN);
        assert_eq!(off.submit(&event(), 0), Err(RequestError::NotAllowed));
        assert!(!off.busy());
        let mut r = requester();
        let bad = Request {
            netfn: 0x05,
            ..event()
        };
        assert_eq!(r.submit(&bad, 0), Err(RequestError::TooLong));
        let long = [0u8; IPMB_MAX_LEN];
        let big = Request {
            data: &long,
            ..event()
        };
        assert_eq!(r.submit(&big, 0), Err(RequestError::TooLong));
        assert!(!r.busy());
        assert_eq!(
            r.submit(&event(), 0),
            Ok(0),
            "the refusals used no sequence"
        );
    }

    /// answer_frame -- the response of the responder `CM` to the request now outstanding in `r`.
    fn answer_bytes(r: &Requester, code: u8, data: &[u8]) -> Vec<u8> {
        let request = ipmb::parse(r.frame()).unwrap();
        let header = ipmb::response_header_for(&request).unwrap();
        let mut buf = [0u8; IPMB_MAX_LEN];
        let n = ipmb::encode_response(&header, code, data, &mut buf).unwrap();
        buf[..n].to_vec()
    }

    /// complete -- sends the outstanding request once and answers it.
    fn complete(r: &mut Requester, now_ms: u32) {
        r.start_tx(Bus::A);
        r.tx_done(true, now_ms, false).unwrap();
        let bytes = answer_bytes(r, 0, &[]);
        assert!(r.on_response(&ipmb::parse(&bytes).unwrap()));
        assert!(r.take_completion().is_some());
    }

    #[test]
    fn the_matching_response_ends_the_request() {
        let mut r = requester();
        r.submit(&event(), 0).unwrap();
        r.start_tx(Bus::A);
        r.tx_done(true, 1, false).unwrap();
        let response = [0x82, 0x14, 0x6A, 0x20, 0x00, 0x02, 0x00, 0xDE];
        assert!(r.on_response(&ipmb::parse(&response).unwrap()));
        assert!(r.busy(), "busy until the completion is taken");
        let done = r.take_completion().unwrap();
        assert_eq!(
            (done.seq, done.netfn, done.cmd),
            (0, NETFN_SE, PLATFORM_EVENT)
        );
        let answer = done.outcome.unwrap();
        assert_eq!((answer.completion_code, answer.data()), (0, &[][..]));
        assert!(!r.busy());
        assert!(r.take_completion().is_none());
    }

    #[test]
    fn a_response_that_differs_in_one_field_does_not_match() {
        let good = {
            let mut r = requester();
            r.submit(&event(), 0).unwrap();
            ipmb::parse(&answer_bytes(&r, 0, &[1])).unwrap().header
        };
        let cases = [
            ("dest", Header { dest: 0x84, ..good }),
            ("src", Header { src: 0x22, ..good }),
            (
                "netfn",
                Header {
                    netfn: 0x07,
                    ..good
                },
            ),
            ("cmd", Header { cmd: 0x03, ..good }),
            ("seq", Header { seq: 1, ..good }),
            (
                "rqLUN",
                Header {
                    dest_lun: 1,
                    ..good
                },
            ),
            ("rsLUN", Header { src_lun: 1, ..good }),
        ];
        for (name, header) in cases {
            let mut r = requester();
            r.submit(&event(), 0).unwrap();
            let mut buf = [0u8; IPMB_MAX_LEN];
            let n = ipmb::encode_response(&header, 0, &[1], &mut buf).unwrap();
            assert!(!r.on_response(&ipmb::parse(&buf[..n]).unwrap()), "{name}");
            assert!(r.take_completion().is_none(), "{name}: still outstanding");
            assert!(r.busy(), "{name}");
        }
    }

    #[test]
    fn busy_too_soon_and_sequence_expiry_refuse_a_submit() {
        let mut r = requester();
        assert_eq!(r.submit(&event(), 0), Ok(0));
        assert_eq!(r.submit(&event(), 100), Err(RequestError::Busy));
        let bytes = answer_bytes(&r, 0, &[]);
        r.start_tx(Bus::A);
        r.tx_done(true, 0, false).unwrap();
        assert!(r.on_response(&ipmb::parse(&bytes).unwrap()));
        assert_eq!(
            r.submit(&event(), 100),
            Err(RequestError::Busy),
            "the completion is not taken yet"
        );
        r.take_completion().unwrap();
        assert_eq!(r.submit(&event(), GAP_MS - 1), Err(RequestError::TooSoon));
        assert_eq!(r.submit(&event(), GAP_MS), Ok(1));
        complete(&mut r, GAP_MS);
        // 64 requests inside 5 s: the 65th would reuse sequence 0 (transmitted at 0 ms).
        for i in 2..SEQ_COUNT as u32 {
            assert_eq!(r.submit(&event(), GAP_MS * i), Ok(i as u8));
            complete(&mut r, GAP_MS * i);
        }
        let now = GAP_MS * SEQ_COUNT as u32;
        assert_eq!(r.submit(&event(), now), Err(RequestError::SeqNotExpired));
        assert_eq!(
            r.submit(&event(), SEQ_EXPIRY_MS - 1),
            Err(RequestError::SeqNotExpired)
        );
        assert_eq!(r.submit(&event(), SEQ_EXPIRY_MS), Ok(0));
    }

    /// send_ok -- hands the outstanding request to a driver on bus A and reports success at
    /// `now_ms`.
    fn send_ok(r: &mut Requester, now_ms: u32) {
        r.start_tx(Bus::A);
        r.tx_done(true, now_ms, false).unwrap();
    }

    #[test]
    fn t4_counts_from_the_first_transmission_and_t9_from_the_latest() {
        let mut r = requester();
        r.submit(&event(), 0).unwrap();
        send_ok(&mut r, 10);
        r.step(10 + TIMEOUT_MS);
        assert!(r.due());
        send_ok(&mut r, 10 + TIMEOUT_MS + 3);
        let bytes = answer_bytes(&r, 0, &[]);
        assert!(r.on_response(&ipmb::parse(&bytes).unwrap()));
        r.take_completion().unwrap();
        let late = 10 + TIMEOUT_MS + 3;
        assert_eq!(
            r.submit(&event(), 10 + GAP_MS - 1),
            Err(RequestError::TooSoon),
            "T4 from the first transmission"
        );
        assert_eq!(r.submit(&event(), 10 + GAP_MS), Ok(1));
        complete(&mut r, 10 + GAP_MS);
        for i in 2..SEQ_COUNT as u32 {
            assert_eq!(r.submit(&event(), 300 + GAP_MS * i), Ok(i as u8));
            complete(&mut r, 300 + GAP_MS * i);
        }
        // Sequence 0 went out first at 10 ms (free again from 5010 ms) and last at `late`: it
        // stays blocked until `late` + T9, so the submit skips it for sequence 1.
        assert_eq!(
            r.submit(&event(), late + SEQ_EXPIRY_MS - 1),
            Ok(1),
            "T9 from the retry, not from the first transmission"
        );
        complete(&mut r, late + SEQ_EXPIRY_MS - 1);
        // 2..=63 went out at 310..=615 ms and stay blocked until 5310 ms or later, 1 just went
        // out, and 0 is free again `late` + T9 after its retry.
        assert_eq!(
            r.submit(&event(), late + SEQ_EXPIRY_MS + GAP_MS),
            Ok(0),
            "free again T9 after the retry"
        );
    }

    #[test]
    fn a_restart_keeps_the_recent_sequence_numbers_blocked_for_a_full_t9() {
        // 64 events inside T9, then a Cold Reset: the new requester continues the counter and
        // books every recent value from its start time.
        let mut old = requester();
        for i in 0..SEQ_COUNT as u32 {
            assert_eq!(old.submit(&event(), GAP_MS * i), Ok(i as u8));
            complete(&mut old, GAP_MS * i);
        }
        let reset_ms = 2752;
        let recent = old.recent_seqs(reset_ms);
        assert_eq!(recent, u64::MAX);
        assert_eq!(old.recent_seqs(GAP_MS * 63 + SEQ_EXPIRY_MS - 1), 1 << 63);
        assert_eq!(old.recent_seqs(GAP_MS * 63 + SEQ_EXPIRY_MS), 0);
        let mut new = Requester::new(true, old.next_seq())
            .with_own_address(OWN)
            .with_recent_seqs(recent, 0);
        assert_eq!(new.submit(&event(), 0), Err(RequestError::SeqNotExpired));
        assert_eq!(
            new.submit(&event(), SEQ_EXPIRY_MS - 1),
            Err(RequestError::SeqNotExpired)
        );
        assert_eq!(new.submit(&event(), SEQ_EXPIRY_MS), Ok(0));
        // Only some recent: the submit skips the blocked ones in order, wrapping.
        let mut partial = Requester::new(true, 62)
            .with_own_address(OWN)
            .with_recent_seqs(1 << 62 | 1 << 63 | 1, 100);
        assert_eq!(partial.submit(&event(), 100), Ok(1));
        assert_eq!(partial.recent_seqs(100), 1 << 62 | 1 << 63 | 1);
    }

    #[test]
    fn a_blocked_next_sequence_is_skipped_and_all_blocked_is_refused() {
        let mut r = Requester::new(true, 5)
            .with_own_address(OWN)
            .with_recent_seqs(0b11 << 5, 0);
        assert_eq!(r.submit(&event(), 0), Ok(7), "5 and 6 are blocked");
        complete(&mut r, 0);
        let mut full = Requester::new(true, 9)
            .with_own_address(OWN)
            .with_recent_seqs(u64::MAX, 0);
        assert_eq!(full.submit(&event(), 1), Err(RequestError::SeqNotExpired));
        assert!(!full.busy());
        assert_eq!(full.submit(&event(), SEQ_EXPIRY_MS), Ok(9));
    }

    #[test]
    fn a_second_cancel_keeps_the_driver_owned_frame_busy_until_tx_done() {
        let mut r = requester();
        r.submit(&event(), 0).unwrap();
        r.start_tx(Bus::A);
        r.cancel();
        r.cancel();
        assert!(r.busy());
        assert_eq!(r.sending_bus(), Some(Bus::A));
        assert_eq!(r.submit(&event(), GAP_MS), Err(RequestError::Busy));
        assert!(r.tx_done(true, 1, true).is_none());
        assert!(!r.busy());
        assert_eq!(r.submit(&event(), 1 + GAP_MS), Ok(1));
    }

    #[test]
    fn the_bus_of_the_latest_transmission_is_kept_until_the_next_request() {
        let mut r = requester();
        assert_eq!(r.last_bus(), None);
        r.submit(&event(), 0).unwrap();
        assert_eq!(r.last_bus(), None);
        r.start_tx(Bus::B);
        r.tx_done(false, 1, true).unwrap();
        assert_eq!(r.last_bus(), Some(Bus::B));
        r.cancel();
        r.submit(&event(), GAP_MS + 1).unwrap();
        assert_eq!(r.last_bus(), None);
    }

    #[test]
    fn the_first_sequence_number_is_a_constructor_input() {
        let mut r = Requester::new(true, 62).with_own_address(OWN);
        assert_eq!(r.next_seq(), 62);
        assert_eq!(r.submit(&event(), 0), Ok(62));
        complete(&mut r, 0);
        assert_eq!(r.submit(&event(), GAP_MS), Ok(63));
        complete(&mut r, GAP_MS);
        assert_eq!(r.submit(&event(), 2 * GAP_MS), Ok(0), "6 bits wrap");
        assert_eq!(
            Requester::new(true, 0xFF).next_seq(),
            63,
            "masked to 6 bits"
        );
    }

    #[test]
    fn a_failed_transmission_waits_t6_before_the_other_bus() {
        let mut r = requester();
        r.submit(&event(), 0).unwrap();
        r.start_tx(Bus::A);
        assert_eq!(r.sending_bus(), Some(Bus::A));
        assert!(r.tx_done(false, 100, true).is_some());
        assert_eq!(r.sending_bus(), None);
        for now in [100, 100 + T6_MIN_MS - 1] {
            r.step(now);
            assert!(!r.due(), "{now}");
        }
        assert!(r.busy());
        r.step(100 + T6_MIN_MS);
        assert!(r.due());
        // A response during the T6 wait still ends the request.
        let mut r = requester();
        r.submit(&event(), 0).unwrap();
        r.start_tx(Bus::A);
        r.tx_done(false, 100, true).unwrap();
        let bytes = answer_bytes(&r, 0, &[]);
        assert!(r.on_response(&ipmb::parse(&bytes).unwrap()));
        // With the other bus disabled the T3 wait applies instead.
        let mut r = requester();
        r.submit(&event(), 0).unwrap();
        r.start_tx(Bus::A);
        r.tx_done(false, 100, false).unwrap();
        r.step(100 + T6_MIN_MS);
        assert!(!r.due());
        r.step(100 + TIMEOUT_MS);
        assert!(r.due());
    }

    #[test]
    fn an_oversize_response_is_not_a_match() {
        let mut r = requester();
        r.submit(&event(), 0).unwrap();
        send_ok(&mut r, 1);
        let request = ipmb::parse(r.frame()).unwrap();
        let header = ipmb::response_header_for(&request).unwrap();
        let mut buf = [0u8; IPMB_MAX_LEN];
        // `parse` cannot return such a frame; the check guards a caller that builds one.
        let big = [0u8; IPMB_RESPONSE_DATA_MAX + 1];
        let oversize = Frame {
            header,
            completion_code: Some(0),
            data: &big,
        };
        assert!(!r.on_response(&oversize));
        assert!(r.take_completion().is_none() && r.busy());
        let ok = [7u8; IPMB_RESPONSE_DATA_MAX];
        let n = ipmb::encode_response(&header, 0, &ok, &mut buf).unwrap();
        assert!(r.on_response(&ipmb::parse(&buf[..n]).unwrap()));
        assert_eq!(r.take_completion().unwrap().outcome.unwrap().data(), ok);
    }

    #[test]
    fn cancel_stops_every_transmission_and_drops_the_completion() {
        // Cancelled while waiting for a due transmission, while waiting for the answer, after
        // the answer: idle at once, nothing due, no completion.
        for stage in 0..3 {
            let mut r = requester();
            r.submit(&event(), 0).unwrap();
            if stage > 0 {
                r.start_tx(Bus::A);
                r.tx_done(true, 1, false).unwrap();
            }
            if stage > 1 {
                let bytes = answer_bytes(&r, 0, &[]);
                assert!(r.on_response(&ipmb::parse(&bytes).unwrap()));
            }
            r.cancel();
            r.step(1 + TIMEOUT_MS);
            assert!(!r.busy() && !r.due(), "stage {stage}");
            assert!(r.take_completion().is_none(), "stage {stage}");
        }
        // Cancelled while the frame is with a driver: busy until that transmission is reported,
        // then idle with no retry, even when it failed and the other bus is enabled.
        let mut r = requester();
        r.submit(&event(), 0).unwrap();
        r.start_tx(Bus::A);
        r.cancel();
        assert!(r.busy() && !r.due());
        assert_eq!(r.submit(&event(), GAP_MS), Err(RequestError::Busy));
        assert!(r.tx_done(false, 1, true).is_none());
        assert!(!r.busy() && !r.due());
        assert_eq!(
            r.submit(&event(), 1 + GAP_MS),
            Ok(1),
            "the next request takes the next seq"
        );
    }
}

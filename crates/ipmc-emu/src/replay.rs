//! replay.rs - replays a captured vector list through an `Emulator` and compares each response
//! with the captured controller's.
//!
//! For every captured request addressed to the emulator: `rx` at its capture time, `poll`, then
//! drain `next_tx` / `tx_done` the way the master driver does. The captured response is the next
//! response frame in the list from the emulator's address with the request's sequence number,
//! NetFn and command.

use crate::emulator::Emulator;
use crate::trace::Hex;
use crate::vectors::Vector;
use core::fmt;
use ipmc_core::commands;
use ipmc_core::dispatch::cc;
use ipmc_core::ipmb::{self, Header, IPMB_MAX_LEN, NETFN_SE};
use ipmc_core::link::{FrameDrop, TxPolicy, TX_RETRIES};
use ipmc_core::queue::Bus;
use ipmc_core::requester;
use ipmc_core::trace::Trace;
use std::collections::BTreeMap;

/// TX_DONE_DELAY_MS -- time from a request to the end of its transmission in the replay, from
/// defs.rs IPMC_EMU_TX_DONE_DELAY_MS.
const TX_DONE_DELAY_MS: u32 = ipmc_defs::IPMC_EMU_TX_DONE_DELAY_MS;
/// REPORT_LINES_MAX -- malformed vector line numbers the report lists, from defs.rs
/// IPMC_EMU_REPORT_LINES_MAX.
const REPORT_LINES_MAX: usize = ipmc_defs::IPMC_EMU_REPORT_LINES_MAX;
/// DRAIN_ATTEMPTS_MAX -- bound on transmissions drained after one request: the first attempt and
/// TX_RETRIES resends of the one response a request produces, with margin for a replayed
/// duplicate.
const DRAIN_ATTEMPTS_MAX: usize = 2 * (TX_RETRIES as usize + 1) + REQUEST_ATTEMPTS_MAX;
/// REQUEST_ATTEMPTS_MAX -- bound on transmissions drained after one request that the IPMC's own
/// requests add: one request with every retry, twice (an event raised by the poll and one the
/// answer to it releases).
const REQUEST_ATTEMPTS_MAX: usize = 2 * requester::MAX_TRANSMISSIONS as usize;
/// EVENT_RECEIVER_ADDR -- IPMB address (8-bit form) of the Event Receiver, the Chassis Manager
/// (VITA 46.11 Rule 7-5).
const EVENT_RECEIVER_ADDR: u8 = 0x20;
/// PLATFORM_EVENT_CMD -- Platform Event Message, command 02h of NetFn Sensor/Event (IPMI v2.0
/// Table G-1).
const PLATFORM_EVENT_CMD: u8 = 0x02;
/// NOT_IMPLEMENTED_NAME -- row name of a command the COMMANDS table does not hold: dispatch
/// answers it C1h, and the names come only from that table (no second name list).
const NOT_IMPLEMENTED_NAME: &str = "(not implemented)";

/// ReplayOptions -- what the replay does besides feeding frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayOptions {
    /// Every Nth transmission is reported failed; 0 reports none failed.
    pub fail_every: u32,
}

/// Class -- how our response compares with the captured one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Byte-identical frames.
    Match,
    /// Same completion code, data differs.
    SameCc,
    /// Different completion codes.
    CcDiff,
    /// We answer C1h (command not implemented), the captured controller did not.
    NotImplemented,
    /// We sent no response.
    Unanswered,
    /// We answered; the capture holds no response to compare with.
    NoCapture,
    /// The request arrived on a bus that is not fitted in this build; not an IPMC fault (a
    /// Chassis Manager resends it on the other bus, Rule 5.1.1-6).
    BusNotFitted,
}

/// Counts -- requests per class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub matched: u32,
    pub same_cc: u32,
    pub cc_diff: u32,
    pub not_implemented: u32,
    pub unanswered: u32,
    pub no_capture: u32,
    pub bus_not_fitted: u32,
}

impl Counts {
    /// counter -- the field that counts `class`.
    fn counter(&mut self, class: Class) -> &mut u32 {
        match class {
            Class::Match => &mut self.matched,
            Class::SameCc => &mut self.same_cc,
            Class::CcDiff => &mut self.cc_diff,
            Class::NotImplemented => &mut self.not_implemented,
            Class::Unanswered => &mut self.unanswered,
            Class::NoCapture => &mut self.no_capture,
            Class::BusNotFitted => &mut self.bus_not_fitted,
        }
    }

    fn add(&mut self, class: Class) {
        *self.counter(class) += 1;
    }

    /// total -- requests in all classes.
    pub fn total(&self) -> u32 {
        self.matched
            + self.same_cc
            + self.cc_diff
            + self.not_implemented
            + self.unanswered
            + self.no_capture
            + self.bus_not_fitted
    }
}

/// Row -- the requests of one (NetFn, command), request NetFn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub netfn: u8,
    pub cmd: u8,
    /// COMMANDS table name, or "(not implemented)" for a command the table does not hold.
    pub name: &'static str,
    pub counts: Counts,
}

/// BusSummary -- the link counters of one bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusSummary {
    pub bus: Bus,
    pub rx_ok: u32,
    /// Frames dropped because the bus was disabled or not built in.
    pub unavailable: u32,
    pub replays: u32,
    pub tx_ok: u32,
    pub tx_fail: u32,
    pub resends: u32,
    pub tx_dropped: u32,
}

/// LinkSummary -- the link layer counters at the end of the replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkSummary {
    pub dispatch_errors: u32,
    pub egress_dropped: u32,
    pub late_responses: u32,
    pub cache_evictions: u32,
    /// Per bus, A then B.
    pub buses: [BusSummary; 2],
}

/// ReplayReport -- the result of one replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayReport {
    /// Captured requests addressed to the emulator.
    pub requests: u32,
    /// Responses completed (each request's response counted once, however often it was resent).
    pub answered: u32,
    /// Requests that produced more than one response.
    pub answered_more_than_once: u32,
    /// One row per (NetFn, command), in numeric order.
    pub rows: Vec<Row>,
    /// All rows added up.
    pub totals: Counts,
    pub link: LinkSummary,
    /// Responses sent on another bus than the captured one (informational).
    pub bus_differs: u32,
    /// Transmissions that were resends, and how many of them went out on the bus other than
    /// the one that failed.
    pub resends: u32,
    pub resends_other_bus: u32,
    /// The response bus policy of the run.
    pub tx_policy: TxPolicy,
    /// The run alternated its transmissions: alternate policy with both buses enabled at start
    /// (Rule 4.1.1-6). With one bus enabled every response leaves on that bus.
    pub alternating: bool,
    /// ReplayOptions::fail_every of the run.
    pub fail_every: u32,
    /// Vector frames `ipmb::parse` rejected (checksum, length, framing).
    pub malformed: u32,
    /// Vector line numbers of the first malformed frames, at most REPORT_LINES_MAX.
    pub malformed_lines: Vec<usize>,
    /// Requests the IPMC originated (Tier 2 and 3), each transmission that was delivered; the
    /// replay answers them as the Chassis Manager's Event Receiver (00h to a Platform Event
    /// for 20h, C1h to any other).
    pub ipmc_requests: u32,
    /// Of `ipmc_requests`, the Platform Event Messages addressed to the Event Receiver.
    pub events: u32,
}

impl ReplayReport {
    /// failures -- the transmit failures reported (tx_fail), as the link counted them.
    pub fn failures(&self) -> u32 {
        self.link.buses.iter().map(|b| b.tx_fail).sum()
    }

    /// link_resends -- the transmissions the link queued again after a failure.
    pub fn link_resends(&self) -> u32 {
        self.link.buses.iter().map(|b| b.resends).sum()
    }

    /// responses_lost -- responses the link gave up on after a failed transmission (tx_dropped).
    pub fn responses_lost(&self) -> u32 {
        self.link.buses.iter().map(|b| b.tx_dropped).sum()
    }

    /// lost_when_injecting -- responses lost, counted only when failures were injected.
    fn lost_when_injecting(&self) -> u32 {
        if self.fail_every > 0 {
            self.responses_lost()
        } else {
            0
        }
    }

    /// fail_reasons -- why the replay did not pass; empty when it passed. Requests of a bus
    /// that is not fitted do not count.
    pub fn fail_reasons(&self) -> Vec<String> {
        let mut reasons = Vec::new();
        if self.requests == 0 {
            reasons.push("no requests addressed to the IPMC".to_string());
        }
        let checks = [
            ("malformed frames", self.malformed),
            ("unanswered", self.totals.unanswered),
            ("answered more than once", self.answered_more_than_once),
            ("dispatch errors", self.link.dispatch_errors),
            ("egress dropped", self.link.egress_dropped),
            ("responses lost", self.lost_when_injecting()),
        ];
        reasons.extend(
            checks
                .iter()
                .filter(|(_, n)| *n > 0)
                .map(|(name, n)| format!("{name} {n}")),
        );
        reasons
    }

    /// passed -- at least one request, every request answered exactly once, no malformed frame,
    /// no dispatch error, no response lost from the egress queue, and no response lost to
    /// injected transmit failures.
    pub fn passed(&self) -> bool {
        self.fail_reasons().is_empty()
            && self.answered + self.totals.bus_not_fitted == self.requests
    }

    /// summary_line -- the one-line form of the result.
    pub fn summary_line(&self) -> String {
        let t = &self.totals;
        format!(
            "{} requests={} answered={} match={} same_cc={} cc_diff={} not_implemented={} \
             unanswered={} no_bus={} bus_differs={} malformed={}",
            if self.passed() { "PASS" } else { "FAIL" },
            self.requests,
            self.answered,
            t.matched,
            t.same_cc,
            t.cc_diff,
            t.not_implemented,
            t.unanswered,
            t.bus_not_fitted,
            self.bus_differs,
            self.malformed
        ) + &self.requests_text()
    }

    /// requests_text -- ` ipmc_requests=N events=M` for the summary line; empty when the IPMC
    /// sent no request of its own (Tier 1), so a Tier-1 line reads as before.
    fn requests_text(&self) -> String {
        if self.ipmc_requests == 0 {
            return String::new();
        }
        format!(
            " ipmc_requests={} events={}",
            self.ipmc_requests, self.events
        )
    }
}

impl fmt::Display for ReplayReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name_width = self
            .rows
            .iter()
            .map(|r| r.name.len())
            .chain(["Command".len()])
            .max()
            .unwrap_or(0);
        writeln!(
            f,
            "{:<name_width$}  {:>5} {:>4}  {:>5} {:>6} {:>6} {:>6} {:>7} {:>6} {:>5} {:>5}",
            "Command",
            "NetFn",
            "Cmd",
            "Reqs",
            "Match",
            "SameCC",
            "CCDiff",
            "NotImpl",
            "Unans",
            "NoCap",
            "NoBus"
        )?;
        let line = |f: &mut fmt::Formatter<'_>,
                    name: &str,
                    netfn: String,
                    cmd: String,
                    c: &Counts|
         -> fmt::Result {
            writeln!(
                f,
                "{:<name_width$}  {:>5} {:>4}  {:>5} {:>6} {:>6} {:>6} {:>7} {:>6} {:>5} {:>5}",
                name,
                netfn,
                cmd,
                c.total(),
                c.matched,
                c.same_cc,
                c.cc_diff,
                c.not_implemented,
                c.unanswered,
                c.no_capture,
                c.bus_not_fitted
            )
        };
        for row in &self.rows {
            line(
                f,
                row.name,
                format!("{:02X}h", row.netfn),
                format!("{:02X}h", row.cmd),
                &row.counts,
            )?;
        }
        line(f, "Total", String::new(), String::new(), &self.totals)?;
        writeln!(f)?;
        writeln!(
            f,
            "Requests {}, answered {}, answered more than once {}",
            self.requests, self.answered, self.answered_more_than_once
        )?;
        if self.alternating {
            writeln!(
                f,
                "responses on another bus than captured: {} (informational: with both buses \
                 enabled the IPMC alternates its transmissions, VITA 46.11 Rule 4.1.1-6; the \
                 captured board answered on the arrival bus)",
                self.bus_differs
            )?;
        } else {
            writeln!(
                f,
                "responses on another bus than captured: {} (informational)",
                self.bus_differs
            )?;
        }
        if self.ipmc_requests > 0 {
            writeln!(
                f,
                "IPMC requests answered as the Event Receiver: {} ({} Platform Events)",
                self.ipmc_requests, self.events
            )?;
        }
        write!(f, "malformed frames: {}", self.malformed)?;
        if !self.malformed_lines.is_empty() {
            let lines: Vec<String> = self.malformed_lines.iter().map(usize::to_string).collect();
            write!(f, " (vector lines {}", lines.join(", "))?;
            if self.malformed_lines.len() < self.malformed as usize {
                write!(f, ", ...")?;
            }
            write!(f, ")")?;
        }
        writeln!(f)?;
        if self.fail_every > 0 {
            writeln!(
                f,
                "injected transmit failures: {}, resent on the other bus: {}, \
                 responses lost: {} (Rule 4.1.1-7)",
                self.failures(),
                self.link_resends(),
                self.responses_lost()
            )?;
        }
        if self.totals.bus_not_fitted > 0 {
            writeln!(
                f,
                "requests on IPMB-B, not fitted in this build: {} (expected: a Chassis Manager \
                 resends them on IPMB-A, Rule 5.1.1-6; that resend belongs to the Phase 7 \
                 Chassis Manager emulator)",
                self.totals.bus_not_fitted
            )?;
        }
        writeln!(
            f,
            "Link: dispatch errors {}, egress dropped {}, late responses {}, cache evictions {}, \
             resends {} ({} on the other bus)",
            self.link.dispatch_errors,
            self.link.egress_dropped,
            self.link.late_responses,
            self.link.cache_evictions,
            self.resends,
            self.resends_other_bus
        )?;
        for b in &self.link.buses {
            writeln!(
                f,
                "IPMB-{}: rx ok {}, unavailable {}, replays {}, tx ok {}, tx fail {}, resends {}, \
                 tx dropped {}",
                b.bus.name(),
                b.rx_ok,
                b.unavailable,
                b.replays,
                b.tx_ok,
                b.tx_fail,
                b.resends,
                b.tx_dropped
            )?;
        }
        if self.passed() {
            write!(f, "PASS")
        } else {
            write!(f, "FAIL: {}", self.fail_reasons().join(", "))
        }
    }
}

/// Sent -- one response the emulator completed: the bus and bytes of the transmission that
/// succeeded.
struct Sent {
    bus: Bus,
    bytes: Vec<u8>,
}

/// replay -- feeds every captured request addressed to `emu` through it and compares the
/// responses with the captured ones. Frame and sensor lines go to `trace`.
pub fn replay(
    emu: &mut Emulator,
    vectors: &[Vector],
    options: &ReplayOptions,
    trace: &mut dyn Trace,
) -> ReplayReport {
    let own = emu.own_address();
    let mut rows: BTreeMap<(u8, u8), Counts> = BTreeMap::new();
    let mut report = ReplayReport {
        requests: 0,
        answered: 0,
        answered_more_than_once: 0,
        rows: Vec::new(),
        totals: Counts::default(),
        link: LinkSummary {
            dispatch_errors: 0,
            egress_dropped: 0,
            late_responses: 0,
            cache_evictions: 0,
            buses: [empty_bus(Bus::A), empty_bus(Bus::B)],
        },
        bus_differs: 0,
        resends: 0,
        resends_other_bus: 0,
        tx_policy: emu.tx_policy(),
        alternating: matches!(emu.tx_policy(), TxPolicy::Alternate)
            && Bus::BOTH
                .iter()
                .all(|&bus| emu.link().bus_state(bus).enabled()),
        fail_every: options.fail_every,
        malformed: 0,
        malformed_lines: Vec::new(),
        ipmc_requests: 0,
        events: 0,
    };
    let mut attempts: u32 = 0;
    for (index, v) in vectors.iter().enumerate() {
        let Ok(request) = ipmb::parse(&v.bytes) else {
            report.malformed += 1;
            if report.malformed_lines.len() < REPORT_LINES_MAX {
                report.malformed_lines.push(v.line);
            }
            trace.line(format_args!(
                "malformed frame at vector line {}: {}",
                v.line,
                Hex(&v.bytes)
            ));
            // A malformed frame that looks like a request to the IPMC still reaches the Link,
            // so its drop counters see it; the verdict comes from `malformed`.
            if looks_like_request_to(&v.bytes, own) {
                let _ = emu.rx(v.bus, &v.bytes, v.time_ms);
            }
            continue;
        };
        if request.header.is_response() || request.header.dest != own {
            continue;
        }
        report.requests += 1;
        trace.line(format_args!(
            "rx IPMB-{} t={} ms: {}",
            v.bus.name(),
            v.time_ms,
            Hex(&v.bytes)
        ));
        let mut bus_not_fitted = false;
        if let Err(drop) = emu.rx(v.bus, &v.bytes, v.time_ms) {
            trace.line(format_args!("rx dropped: {}", drop.name()));
            // The drop reason covers a disabled and an unfitted bus alike; the link's bus
            // state tells which.
            bus_not_fitted =
                drop == FrameDrop::BusUnavailable && !emu.link().bus_state(v.bus).fitted;
        }
        emu.poll(v.time_ms, trace);
        let sent = drain(emu, v.time_ms, options, &mut attempts, &mut report, trace);

        let captured = find_captured(vectors, index, &request.header);
        let class = match (sent.first(), captured) {
            (None, _) if bus_not_fitted => Class::BusNotFitted,
            (None, _) => Class::Unanswered,
            (Some(_), None) => Class::NoCapture,
            (Some(ours), Some(theirs)) => {
                if ours.bus != theirs.bus {
                    report.bus_differs += 1;
                }
                classify(&ours.bytes, &theirs.bytes)
            }
        };
        report.answered += u32::try_from(sent.len()).unwrap_or(u32::MAX);
        if sent.len() > 1 {
            report.answered_more_than_once += 1;
        }
        trace.line(format_args!(
            "{:02X}h/{:02X}h seq {:02X}h -> {:?}",
            request.header.netfn, request.header.cmd, request.header.seq, class
        ));
        rows.entry((request.header.netfn, request.header.cmd))
            .or_default()
            .add(class);
    }
    for ((netfn, cmd), counts) in rows {
        report.totals.matched += counts.matched;
        report.totals.same_cc += counts.same_cc;
        report.totals.cc_diff += counts.cc_diff;
        report.totals.not_implemented += counts.not_implemented;
        report.totals.unanswered += counts.unanswered;
        report.totals.no_capture += counts.no_capture;
        report.totals.bus_not_fitted += counts.bus_not_fitted;
        report.rows.push(Row {
            netfn,
            cmd,
            name: commands::find(netfn, cmd).map_or(NOT_IMPLEMENTED_NAME, |c| c.name),
            counts,
        });
    }
    let link = emu.link();
    report.link.dispatch_errors = link.dispatch_errors();
    report.link.egress_dropped = link.egress_dropped();
    report.link.late_responses = link.late_responses();
    report.link.cache_evictions = link.evictions();
    for summary in &mut report.link.buses {
        let c = link.counters(summary.bus);
        summary.rx_ok = c.rx_ok;
        summary.unavailable = c.bus_unavailable;
        summary.replays = c.replays;
        summary.tx_ok = c.tx_ok;
        summary.tx_fail = c.tx_fail;
        summary.resends = c.resends;
        summary.tx_dropped = c.tx_dropped;
    }
    report
}

fn empty_bus(bus: Bus) -> BusSummary {
    BusSummary {
        bus,
        rx_ok: 0,
        unavailable: 0,
        replays: 0,
        tx_ok: 0,
        tx_fail: 0,
        resends: 0,
        tx_dropped: 0,
    }
}

/// is_request_frame -- the classification of a frame the IPMC transmits, `ipmb::is_request_frame`:
/// an even NetFn is a request the IPMC originated (Tier 2 and 3), an odd one the response to a
/// request it received.
pub use ipmc_core::ipmb::is_request_frame;

/// looks_like_request_to -- a frame whose first byte is `own` and that is a request; used only
/// for frames `ipmb::parse` rejected.
fn looks_like_request_to(bytes: &[u8], own: u8) -> bool {
    bytes.first() == Some(&own) && is_request_frame(bytes)
}

/// event_receiver_answer -- the completion code the replay answers the IPMC's request `frame`
/// with, as the captured Chassis Manager's Event Receiver: 00h for a Platform Event addressed to
/// 20h, C1h for anything else. Also whether it was a Platform Event.
fn event_receiver_answer(request: &ipmb::Frame) -> (u8, bool) {
    let h = &request.header;
    if h.dest == EVENT_RECEIVER_ADDR && h.netfn == NETFN_SE && h.cmd == PLATFORM_EVENT_CMD {
        (cc::OK, true)
    } else {
        (cc::INVALID_CMD, false)
    }
}

/// answer_request -- feeds the Event Receiver's response to the IPMC request `bytes` (delivered
/// on `bus`) back through the Link on that bus at `at_ms`, polls, and counts it in `report`.
fn answer_request(
    emu: &mut Emulator,
    bus: Bus,
    bytes: &[u8],
    at_ms: u32,
    report: &mut ReplayReport,
    trace: &mut dyn Trace,
) {
    let Ok(request) = ipmb::parse(bytes) else {
        return;
    };
    let (code, is_event) = event_receiver_answer(&request);
    report.ipmc_requests += 1;
    report.events += u32::from(is_event);
    let Ok(header) = ipmb::response_header_for(&request) else {
        return;
    };
    let mut out = [0u8; IPMB_MAX_LEN];
    let Ok(len) = ipmb::encode_response(&header, code, &[], &mut out) else {
        return;
    };
    let frame = out.get(..len).unwrap_or_default();
    trace.line(format_args!(
        "rx IPMB-{} t={} ms: {}",
        bus.name(),
        at_ms,
        Hex(frame)
    ));
    if let Err(drop) = emu.rx(bus, frame, at_ms) {
        trace.line(format_args!("rx dropped: {}", drop.name()));
    }
    emu.poll(at_ms, trace);
}

/// drain -- takes every frame the emulator wants transmitted, reporting each transmission done
/// at `request_ms` + TX_DONE_DELAY_MS (failed for every `fail_every`th one). Returns the
/// responses completed: one entry per response that a transmission delivered, with the bus and
/// bytes of that successful attempt. A response whose every attempt failed has no entry (it is
/// lost, and the link counts it in tx_dropped). A request the IPMC originated (`is_request_frame`)
/// is no response: it is answered as the Event Receiver (`answer_request`) and counted in
/// `report`.
fn drain(
    emu: &mut Emulator,
    request_ms: u32,
    options: &ReplayOptions,
    attempts: &mut u32,
    report: &mut ReplayReport,
    trace: &mut dyn Trace,
) -> Vec<Sent> {
    let done_ms = request_ms.saturating_add(TX_DONE_DELAY_MS);
    let mut sent: Vec<Sent> = Vec::new();
    // The bus of the failed attempt the next transmission resends; None when the next
    // transmission starts a new response.
    let mut failed_bus: Option<Bus> = None;
    for _ in 0..DRAIN_ATTEMPTS_MAX {
        let Some((bus, frame)) = emu.next_tx() else {
            break;
        };
        let bytes = frame.to_vec();
        *attempts += 1;
        let ok = options.fail_every == 0 || !(*attempts).is_multiple_of(options.fail_every);
        trace.line(format_args!(
            "tx IPMB-{} {}: {}",
            bus.name(),
            if ok { "ok" } else { "fail" },
            Hex(&bytes)
        ));
        if let Some(failed) = failed_bus {
            report.resends += 1;
            if failed != bus {
                report.resends_other_bus += 1;
            }
        }
        if emu.tx_done(bus, ok, done_ms, trace).is_err() {
            break;
        }
        if ok && is_request_frame(&bytes) {
            // Its own request, not an answer: the Event Receiver answers it one step later.
            answer_request(emu, bus, &bytes, done_ms.saturating_add(1), report, trace);
        } else if ok {
            sent.push(Sent { bus, bytes });
        }
        failed_bus = (!ok).then_some(bus);
    }
    sent
}

/// find_captured -- the captured response to the request at `index`: the next response frame
/// from the request's destination to its source with the same sequence number, NetFn and
/// command. The search stops at a later copy of the same request.
fn find_captured<'a>(vectors: &'a [Vector], index: usize, req: &Header) -> Option<&'a Vector> {
    for v in vectors.iter().skip(index + 1) {
        let Ok(frame) = ipmb::parse(&v.bytes) else {
            continue;
        };
        let h = &frame.header;
        let same_exchange = h.seq == req.seq && h.cmd == req.cmd;
        if h.is_response() {
            if same_exchange && h.netfn == req.netfn + 1 && h.src == req.dest && h.dest == req.src {
                return Some(v);
            }
        } else if same_exchange && h.netfn == req.netfn && h.dest == req.dest {
            return None;
        }
    }
    None
}

/// classify -- compares our response with the captured one (both whole frames).
fn classify(ours: &[u8], captured: &[u8]) -> Class {
    if ours == captured {
        return Class::Match;
    }
    let our_cc = ipmb::parse(ours).ok().and_then(|f| f.completion_code);
    let their_cc = ipmb::parse(captured).ok().and_then(|f| f.completion_code);
    if our_cc == Some(cc::INVALID_CMD) && their_cc != Some(cc::INVALID_CMD) {
        Class::NotImplemented
    } else if our_cc == their_cc {
        Class::SameCc
    } else {
        Class::CcDiff
    }
}

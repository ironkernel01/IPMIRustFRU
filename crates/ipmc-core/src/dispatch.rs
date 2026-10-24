//! dispatch.rs - command dispatcher: request in, response frame queued for transmission.
//!
//! ```text
//! Frame -> commands::find_in -> handler(ctx, req, data) -> ipmb::encode_response -> EgressQueue
//! ```
//!
//! Only LUN 0 is supported: a request to any other destination LUN gets C2h (IPMI v2.0
//! Table 5-2), before the table lookup, so no handler sees a LUN it does not serve.
//! Unknown (netFn, cmd) gets C1h. Group Extension (2Ch) requests are checked for the VSO
//! identifier and every 2Dh response starts with it, in `run_request` only.

use crate::addr::AddrInfo;
use crate::commands::{self, Command, COMMANDS};
use crate::fru_state::{FruMachine, InitialChecks, Inputs, Transition};
use crate::ipmb::{self, Frame, IpmbError, NETFN_GROUP_EXT};
use crate::link::{BusPair, LinkConfig, LinkView, PendingIpmbState};
use crate::queue::{Bus, EgressQueue, QueueError};
use crate::sdr::SdrRepository;
use crate::selftest::{self, SelfTest};
use crate::sensor::{
    self, EncodeError, PowerState, SensorSnapshot, SENSOR_FRU_HEALTH, SENSOR_FRU_STATE,
    SENSOR_FRU_TEMPERATURE, SENSOR_FRU_VOLTAGE, SENSOR_PAYLOAD_TEST_RESULTS,
    SENSOR_PAYLOAD_TEST_STATUS, SENSOR_SYSTEM_IPMB_LINK,
};
use crate::trace::Trace;
use ipmc_fru::FruStore;

/// cc -- completion codes.
pub mod cc {
    /// OK -- command completed normally (IPMI v2.0 Table 5-2).
    pub const OK: u8 = 0x00;
    /// WRITE_PROTECTED -- write-protected offset, Write FRU Data only (IPMI v2.0 Table 34-4;
    /// VITA 46.11 Rule 8-5).
    pub const WRITE_PROTECTED: u8 = 0x80;
    /// NODE_BUSY -- node busy (IPMI v2.0 Table 5-2).
    pub const NODE_BUSY: u8 = 0xC0;
    /// INVALID_CMD -- invalid command (IPMI v2.0 Table 5-2).
    pub const INVALID_CMD: u8 = 0xC1;
    /// INVALID_FOR_LUN -- command invalid for the given LUN (IPMI v2.0 Table 5-2).
    pub const INVALID_FOR_LUN: u8 = 0xC2;
    /// TIMEOUT -- timeout while processing the command (IPMI v2.0 Table 5-2).
    pub const TIMEOUT: u8 = 0xC3;
    /// OUT_OF_SPACE -- out of space (IPMI v2.0 Table 5-2).
    pub const OUT_OF_SPACE: u8 = 0xC4;
    /// RESERVATION_INVALID -- reservation canceled or invalid reservation ID (IPMI v2.0 Table 5-2).
    pub const RESERVATION_INVALID: u8 = 0xC5;
    /// REQ_TRUNCATED -- request data truncated (IPMI v2.0 Table 5-2).
    pub const REQ_TRUNCATED: u8 = 0xC6;
    /// REQ_LEN_INVALID -- request data length invalid (IPMI v2.0 Table 5-2).
    pub const REQ_LEN_INVALID: u8 = 0xC7;
    /// REQ_LEN_EXCEEDED -- request data field length limit exceeded (IPMI v2.0 Table 5-2).
    pub const REQ_LEN_EXCEEDED: u8 = 0xC8;
    /// PARAM_OUT_OF_RANGE -- parameter out of range (IPMI v2.0 Table 5-2).
    pub const PARAM_OUT_OF_RANGE: u8 = 0xC9;
    /// CANT_RETURN_BYTES -- cannot return the number of requested data bytes (IPMI v2.0 Table 5-2).
    pub const CANT_RETURN_BYTES: u8 = 0xCA;
    /// NOT_PRESENT -- requested sensor, data or record not present (IPMI v2.0 Table 5-2).
    pub const NOT_PRESENT: u8 = 0xCB;
    /// INVALID_DATA_FIELD -- invalid data field in request (IPMI v2.0 Table 5-2).
    pub const INVALID_DATA_FIELD: u8 = 0xCC;
    /// ILLEGAL_FOR_SENSOR -- command illegal for the specified sensor or record type (IPMI v2.0 Table 5-2).
    pub const ILLEGAL_FOR_SENSOR: u8 = 0xCD;
    /// CANT_RESPOND -- command response could not be provided (IPMI v2.0 Table 5-2).
    pub const CANT_RESPOND: u8 = 0xCE;
    /// DUPLICATE -- cannot execute a duplicated request (IPMI v2.0 Table 5-2).
    pub const DUPLICATE: u8 = 0xCF;
    /// DESTINATION_UNAVAILABLE -- destination unavailable: the addressed FRU is not present
    /// (VITA 46.11 Rule 4.1.1-18).
    pub const DESTINATION_UNAVAILABLE: u8 = 0xD3;
    /// FRU_STATE -- FRU state does not allow the command (VITA 46.11 Rule 4.1.2-5).
    pub const FRU_STATE: u8 = 0xD5;
    /// UNSPECIFIED -- unspecified error (IPMI v2.0 Table 5-2).
    pub const UNSPECIFIED: u8 = 0xFF;
}

/// Context -- state handed to every handler. The egress queue and the trace sink are arguments of
/// `dispatch` instead: handlers cannot reach either, which keeps "a handler writes only its
/// data bytes" true by construction.
///
/// Every field has one owner:
/// - `addr`: written once at start-up by `addr::resolve`; read-only afterwards.
/// - `sensors`: the latest sensor snapshot; the firmware writes it each sample through
///   `update_sensors`, handlers read it (Get Sensor Reading sensors 2..=4).
/// - `fru`: the RAM FRU store (ipmc-fru); Write FRU Data is its only writer (and marks it dirty
///   for the flash log), apart from `set_fru`, which the firmware calls at boot and for FRU
///   RESTORE.
/// - `sdr`: the Device SDR repository and its reservation; Reserve Device SDR Repository is
///   the only writer.
/// - `self_test`: the result of the controller self test; `Context::new` sets it and an accepted
///   Write FRU Data refreshes it (`selftest`); Get Self Test Results reads it and the FRU initial
///   checks gate M0 to M1 on it.
/// - `fru_state`: the FRU 0 state machine; `update_sensors` is its only writer, Get Sensor
///   Reading sensor 0 reads it.
/// - `link`: a read-only view of the link layer (bus states, build configuration). `Link::poll`
///   refreshes it before every dispatch; a handler reads it and never writes it.
/// - `pending_ipmb`: a handler's request to the link layer, the Set IPMB State hand-off (see
///   `link::PendingIpmbState`): the handler validates the change against `link` with
///   `link::plan_ipmb_state`, leaves it here, and `Link::poll` applies and clears it right after
///   the handler returns, before the response is queued.
pub struct Context {
    /// Resolved geographic address and NVMRO state.
    pub addr: AddrInfo,
    /// Latest sensor snapshot (`SensorSnapshot::EMPTY` before the first sample).
    pub sensors: SensorSnapshot,
    /// RAM FRU store of FRU 0.
    pub fru: FruStore,
    /// Device SDR repository and reservation state.
    pub sdr: SdrRepository,
    /// Result of the latest self test (IPMI v2.0 Table 20-5).
    pub self_test: SelfTest,
    /// Operational State of FRU 0.
    pub fru_state: FruMachine,
    /// Read-only view of the link layer for the current dispatch.
    pub link: LinkView,
    /// Set IPMB State change for the link layer to apply after the handler returns.
    pub pending_ipmb: Option<PendingIpmbState>,
}

impl Context {
    /// new -- the start-up context for the resolved address `addr`: the FRU image and the SDR
    /// repository built (owner = this controller's IPMB address), the startup self test run on
    /// them, no sensor sample yet, FRU state M0 and the link view of a fresh link with the
    /// ipmi_defs.h configuration.
    pub fn new(addr: AddrInfo) -> Context {
        let fru = FruStore::new();
        let sdr = SdrRepository::new(addr.ipmb_address);
        let self_test = selftest::run(fru.bytes(), &sdr);
        Context {
            addr,
            sensors: SensorSnapshot::EMPTY,
            fru,
            sdr,
            self_test,
            fru_state: FruMachine::new(),
            link: LinkView::power_up(LinkConfig::from_defs(), addr.usable()),
            pending_ipmb: None,
        }
    }

    /// set_fru -- replaces the FRU store (the image loaded from flash at boot, or the built image
    /// after FRU RESTORE) and reruns the self test on it, so Get Self Test Results and the FRU
    /// initial checks (Rule 4.1.5.4.1-9) judge the image now in use: a corrupt stored image
    /// gives 57h with the FRU bit and keeps the FRU in M0 (owner decision 8).
    pub fn set_fru(&mut self, fru: FruStore) {
        self.fru = fru;
        self.self_test = selftest::run(self.fru.bytes(), &self.sdr);
    }

    /// initial_checks -- the FRU initial criteria (Rule 4.1.5.4.1-2) as they stand now.
    /// Management power is assumed valid on this board (the IPMC runs from it); the address
    /// parity and a nonzero GA come from `addr`; the SDR repository exists once `new` built it;
    /// the self test is the result Get Self Test Results reports (`self_test`).
    pub fn initial_checks(&self) -> InitialChecks {
        InitialChecks {
            management_power_valid: true,
            address_parity_valid: self.addr.usable(),
            sdr_repository_ready: self.sdr.record_count() > 0,
            self_test_passed: self.self_test.passed(),
        }
    }

    /// update_sensors -- stores `snapshot` and steps the FRU state machine with it: payload power
    /// is valid when the power state machine is On, which means PGOOD was seen (owner decision
    /// 3), and the shutdown that ends M6 is complete when the snapshot says PWR_EN is released
    /// and PGOOD is read deasserted (`SensorSnapshot::payload_shut_down`). The firmware and the
    /// emulator call it once per sensor sample. A state change writes one event line to `trace`,
    /// `FRU state: M0 IPMC Inactive -> M1 FRU Inactive`, whatever IPMC_IPMI_TRACE says (it is an
    /// event, not a request trace); this is the only place that line is written. Returns the FRU
    /// state transition, if one happened.
    pub fn update_sensors(
        &mut self,
        now_ms: u32,
        snapshot: &SensorSnapshot,
        trace: &mut dyn Trace,
    ) -> Option<Transition> {
        self.sensors = *snapshot;
        let inputs = Inputs {
            checks: self.initial_checks(),
            payload_power_valid: snapshot.power == PowerState::On,
            payload_shut_down: snapshot.payload_shut_down,
        };
        let change = self.fru_state.step(now_ms, &inputs)?;
        trace.line(format_args!(
            "FRU state: {} -> {}",
            change.from.name(),
            change.to.name()
        ));
        Some(change)
    }

    /// read_sensor -- the Get Sensor Reading data bytes of sensor `number` (reading, status,
    /// state; VITA Table 10.2-1 sensors 0..=6) written to the front of `out`. The one function
    /// behind the Get Sensor Reading handler and the console; `buses` is the link state the IPMB
    /// Link sensor reports (a handler passes its `link` view, the console the live link):
    /// - 0 FRU State from the FRU state machine (Table 10.2.1-1);
    /// - 1 IPMB Link from `buses` (Table 10.2.2-1);
    /// - 2 FRU Health, 3 FRU Voltage, 4 FRU Temperature from the latest sensor snapshot;
    /// - 5 Payload Test Results: unavailable, no payload test has run (Table 10.2.6-1);
    /// - 6 Payload Test Status: not in progress (Table 10.2.7-1).
    ///
    /// Returns the byte count, Err(NotPresent) for a number outside 0..=6 and Err(OutputTooSmall)
    /// when `out` cannot hold a reading.
    pub fn read_sensor(
        &self,
        number: u8,
        buses: &BusPair,
        out: &mut [u8],
    ) -> Result<usize, SensorReadError> {
        let encoded = match number {
            SENSOR_FRU_STATE => sensor::encode_fru_state(self.fru_state.state().mask(), out),
            SENSOR_SYSTEM_IPMB_LINK => sensor::encode_link(&buses.sensor_data(), out),
            SENSOR_FRU_HEALTH | SENSOR_FRU_VOLTAGE | SENSOR_FRU_TEMPERATURE => {
                sensor::encode_sensor_reading(number, &self.sensors, out)
            }
            SENSOR_PAYLOAD_TEST_RESULTS => sensor::encode_payload_test_results(out),
            SENSOR_PAYLOAD_TEST_STATUS => sensor::encode_payload_test_status(false, out),
            _ => return Err(SensorReadError::NotPresent),
        };
        encoded.map_err(|error| match error {
            EncodeError::BufferTooShort => SensorReadError::OutputTooSmall,
            EncodeError::UnsupportedSensor => SensorReadError::NotPresent,
        })
    }
}

/// SensorReadError -- why `Context::read_sensor` produced no reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorReadError {
    /// The sensor number is not one of the seven mandatory sensors.
    NotPresent,
    /// The output buffer is shorter than a reading.
    OutputTooSmall,
}

impl SensorReadError {
    /// name -- log name of the error.
    pub const fn name(self) -> &'static str {
        match self {
            SensorReadError::NotPresent => "NotPresent",
            SensorReadError::OutputTooSmall => "OutputTooSmall",
        }
    }
}

/// Handler -- A command handler. Ok(n) = n response data bytes written after the completion code;
/// Err(code) = completion code returned with no data. For a Group Extension request the
/// frame's data and the output slice both exclude the VSO identifier byte (see `dispatch`).
pub type Handler = fn(&mut Context, &Frame, &mut [u8]) -> Result<usize, u8>;

/// VSO_IDENTIFIER -- VSO identifier, the Group Extension defining body of VITA 46.11 (VITA 46.11 section
/// 10.1.2). IPMI v2.0 section 5.1: the first data byte of a Group Extension request and
/// response names the defining body. No other defining body is supported.
pub const VSO_IDENTIFIER: u8 = 0x03;

/// Outcome -- how a request was resolved; selects the trace line shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A table row matched; carries its name.
    Handled(&'static str),
    /// No row matches (netFn, cmd): C1h.
    NoHandler,
    /// A Group Extension request for a defining body other than VSO: C1h.
    BadVso,
    /// A Group Extension request with no data, so no defining body: C7h.
    NoDefiningBody,
    /// The destination LUN is not 0: C2h; carries the LUN asked for.
    BadLun(u8),
}

/// Built -- what `build_response_with` produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Built {
    /// Frame length written to the output buffer.
    pub len: usize,
    /// Completion code of the response.
    pub completion_code: u8,
    /// Response data bytes after the completion code (the VSO byte of a 2Dh response counts).
    pub data_len: usize,
    /// How the request was resolved.
    pub outcome: Outcome,
}

/// DispatchError -- why dispatch produced no queued response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchError {
    /// The frame builder failed (not a request, output buffer too small, handler overflow).
    Frame(IpmbError),
    /// The response was built but the egress queue rejected it.
    Queue(QueueError),
}

impl DispatchError {
    /// name -- log name of the underlying error.
    pub const fn name(self) -> &'static str {
        match self {
            DispatchError::Frame(e) => e.name(),
            DispatchError::Queue(e) => e.name(),
        }
    }
}

/// SUPPORTED_LUN -- the only destination LUN this controller serves (the sensor, FRU and
/// application commands live on LUN 0; VITA 46.11 mandatory sensors are on LUN 0).
const SUPPORTED_LUN: u8 = 0;

/// unsupported_lun -- the outcome for a request whose destination LUN is not SUPPORTED_LUN.
fn unsupported_lun(req: &Frame) -> Option<(u8, Outcome)> {
    let lun = req.header.dest_lun;
    (lun != SUPPORTED_LUN).then_some((cc::INVALID_FOR_LUN, Outcome::BadLun(lun)))
}

/// run_command -- looks up the row and runs its handler into `data`.
/// Returns (completion code, outcome, data length); TooLong when the handler reports more
/// bytes than `data` holds.
fn run_command(
    table: &[Command],
    ctx: &mut Context,
    req: &Frame,
    data: &mut [u8],
) -> Result<(u8, Outcome, usize), IpmbError> {
    let Some(row) = commands::find_in(table, req.header.netfn, req.header.cmd) else {
        return Ok((cc::INVALID_CMD, Outcome::NoHandler, 0));
    };
    let outcome = Outcome::Handled(row.name);
    match (row.handler)(ctx, req, data) {
        Ok(len) if len <= data.len() => Ok((cc::OK, outcome, len)),
        Ok(_) => Err(IpmbError::TooLong),
        Err(code) => Ok((code, outcome, 0)),
    }
}

/// run_request -- runs a request into `data`, the response data buffer.
/// A Group Extension request gets the defining-body handling here, the only place that writes
/// the response's defining-body byte. IPMI v2.0 Table 5-1 (NetFn 2Ch/2Dh): the first data byte
/// of a request AND of its response names the defining body, so data[0] echoes the request's
/// byte on every path (03h for VSO). A request with no data has no defining body: C7h with no
/// data (IPMI Table 5-2; section 5.3.1 lets the response stop after the completion code). A
/// defining body other than VSO: C1h. Otherwise the handler sees the request data after the
/// VSO byte (a copy of the Frame with `data` re-sliced) and the buffer after data[0], and the
/// returned length includes the VSO byte. A destination LUN other than 0 is answered C2h here
/// before any lookup (a 2Dh response still starts with the defining-body byte).
fn run_request(
    table: &[Command],
    ctx: &mut Context,
    req: &Frame,
    data: &mut [u8],
) -> Result<(u8, Outcome, usize), IpmbError> {
    if req.header.netfn != NETFN_GROUP_EXT {
        if let Some((code, outcome)) = unsupported_lun(req) {
            return Ok((code, outcome, 0));
        }
        return run_command(table, ctx, req, data);
    }
    let Some((body_slot, handler_out)) = data.split_first_mut() else {
        return Err(IpmbError::TooLong);
    };
    let Some((&defining_body, body)) = req.data.split_first() else {
        // No defining body to echo; C2h for another LUN still comes before C7h.
        let (code, outcome) =
            unsupported_lun(req).unwrap_or((cc::REQ_LEN_INVALID, Outcome::NoDefiningBody));
        return Ok((code, outcome, 0));
    };
    *body_slot = defining_body;
    if let Some((code, outcome)) = unsupported_lun(req) {
        return Ok((code, outcome, 1));
    }
    if defining_body != VSO_IDENTIFIER {
        return Ok((cc::INVALID_CMD, Outcome::BadVso, 1));
    }
    let view = Frame { data: body, ..*req };
    let (code, outcome, len) = run_command(table, ctx, &view, handler_out)?;
    Ok((code, outcome, len.checked_add(1).ok_or(IpmbError::TooLong)?))
}

/// build_response_with -- builds the complete response frame for `req` into `out_frame`
/// using `table`. The single frame builder behind `dispatch` and the tests.
/// 1. Builds the response header (NotARequest when `req` is a response).
/// 2. Checks `out_frame` holds a whole frame (OutBufferTooSmall) BEFORE any handler runs, so
///    a handler's side effects never happen for a response that cannot be delivered.
/// 3. Runs the request; encodes the response.
pub fn build_response_with(
    table: &[Command],
    ctx: &mut Context,
    req: &Frame,
    out_frame: &mut [u8],
) -> Result<Built, IpmbError> {
    let header = ipmb::response_header_for(req)?;
    if out_frame.len() < ipmb::IPMB_MAX_LEN {
        return Err(IpmbError::OutBufferTooSmall);
    }
    let mut data = [0u8; ipmb::IPMB_RESPONSE_DATA_MAX];
    let (completion_code, outcome, data_len) = run_request(table, ctx, req, &mut data)?;
    let payload = data.get(..data_len).ok_or(IpmbError::TooLong)?;
    let len = ipmb::encode_response(&header, completion_code, payload, out_frame)?;
    Ok(Built {
        len,
        completion_code,
        data_len,
        outcome,
    })
}

/// trace_request -- one PIC32-style line for a dispatched request, when IPMC_IPMI_TRACE is 1.
/// `FN(06h) CMD(01h) Get Device ID -> CC 00h, 15 bytes`
/// `FN(06h) CMD(55h) -> C1h (no handler)`
fn trace_request(trace: &mut dyn Trace, req: &Frame, built: &Built) {
    if ipmc_defs::IPMC_IPMI_TRACE != 1 {
        return;
    }
    let (netfn, cmd, code) = (req.header.netfn, req.header.cmd, built.completion_code);
    match built.outcome {
        Outcome::Handled(name) => trace.line(format_args!(
            "FN({netfn:02X}h) CMD({cmd:02X}h) {name} -> CC {code:02X}h, {} bytes",
            built.data_len
        )),
        Outcome::NoHandler => trace.line(format_args!(
            "FN({netfn:02X}h) CMD({cmd:02X}h) -> {code:02X}h (no handler)"
        )),
        Outcome::BadVso => trace.line(format_args!(
            "FN({netfn:02X}h) CMD({cmd:02X}h) -> {code:02X}h (defining body not VSO)"
        )),
        Outcome::NoDefiningBody => trace.line(format_args!(
            "FN({netfn:02X}h) CMD({cmd:02X}h) -> {code:02X}h (no defining body byte)"
        )),
        Outcome::BadLun(lun) => trace.line(format_args!(
            "FN({netfn:02X}h) CMD({cmd:02X}h) -> {code:02X}h (LUN {lun} not supported)"
        )),
    }
}

/// dispatch_with -- answers `req` using `table`: builds the response, traces the request and
/// pushes the frame onto `queue` with the bus the request arrived on and its arrival time
/// (`arrival_ms`, the request STOP); the link layer chooses the transmit bus later.
/// The queue is checked for a free slot BEFORE any handler runs (a handler has side effects;
/// a response that cannot be queued must not cause them): a full queue runs nothing, counts a
/// drop and returns Err(Queue(Full)).
/// Err(Frame(..)) = nothing built (no handler ran for OutBufferTooSmall/NotARequest);
/// Err(Queue(..)) = not queued.
pub fn dispatch_with(
    table: &[Command],
    ctx: &mut Context,
    req: &Frame,
    arrival: Bus,
    arrival_ms: u32,
    queue: &mut EgressQueue,
    trace: &mut dyn Trace,
) -> Result<(), DispatchError> {
    queue.reserve().map_err(DispatchError::Queue)?;
    let mut frame = [0u8; ipmb::IPMB_MAX_LEN];
    let built = build_response_with(table, ctx, req, &mut frame).map_err(DispatchError::Frame)?;
    trace_request(trace, req, &built);
    let bytes = frame
        .get(..built.len)
        .ok_or(DispatchError::Frame(IpmbError::TooLong))?;
    queue
        .push(bytes, arrival, arrival_ms)
        .map_err(DispatchError::Queue)
}

/// dispatch -- `dispatch_with` over the COMMANDS table.
pub fn dispatch(
    ctx: &mut Context,
    req: &Frame,
    arrival: Bus,
    arrival_ms: u32,
    queue: &mut EgressQueue,
    trace: &mut dyn Trace,
) -> Result<(), DispatchError> {
    dispatch_with(COMMANDS, ctx, req, arrival, arrival_ms, queue, trace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addr;
    use crate::commands::cmd;
    use crate::ipmb::{NETFN_APP, NETFN_STORAGE};
    use crate::trace::{NoTrace, RecordingTrace};
    use core::sync::atomic::{AtomicBool, Ordering};

    fn context() -> Context {
        Context::new(addr::resolve(0x7F))
    }

    fn request_bytes(netfn: u8, cmd: u8, data: &[u8]) -> ([u8; ipmb::IPMB_MAX_LEN], usize) {
        let header = ipmb::Header {
            dest: 0x82,
            netfn,
            dest_lun: 0,
            src: 0x20,
            seq: 5,
            src_lun: 0,
            cmd,
        };
        let mut buf = [0u8; ipmb::IPMB_MAX_LEN];
        let n = ipmb::encode_request(&header, data, &mut buf).unwrap();
        (buf, n)
    }

    /// Dispatches one request through `table`; returns the queued response bytes and trace.
    fn run(table: &[Command], netfn: u8, cmd: u8, data: &[u8]) -> (Vec<u8>, Vec<String>) {
        let (buf, n) = request_bytes(netfn, cmd, data);
        let req = ipmb::parse(&buf[..n]).unwrap();
        let mut queue = EgressQueue::new();
        let mut trace = RecordingTrace::default();
        dispatch_with(
            table,
            &mut context(),
            &req,
            Bus::B,
            0,
            &mut queue,
            &mut trace,
        )
        .unwrap();
        let frame = queue.pop().unwrap();
        assert_eq!(frame.bus, Bus::B);
        assert!(queue.is_empty());
        (frame.as_bytes().to_vec(), trace.lines)
    }

    fn echo_handler(_c: &mut Context, r: &Frame, out: &mut [u8]) -> Result<usize, u8> {
        // Echoes the request data it was given, to show where the VSO byte was removed.
        out.get_mut(..r.data.len())
            .ok_or(cc::UNSPECIFIED)?
            .copy_from_slice(r.data);
        Ok(r.data.len())
    }

    fn failing_handler(_c: &mut Context, _r: &Frame, _o: &mut [u8]) -> Result<usize, u8> {
        Err(cc::PARAM_OUT_OF_RANGE)
    }

    fn overlong_handler(_c: &mut Context, _r: &Frame, _o: &mut [u8]) -> Result<usize, u8> {
        Ok(ipmb::IPMB_RESPONSE_DATA_MAX + 1)
    }

    static RAN: AtomicBool = AtomicBool::new(false);
    fn recording_handler(_c: &mut Context, _r: &Frame, _o: &mut [u8]) -> Result<usize, u8> {
        RAN.store(true, Ordering::Relaxed);
        Ok(0)
    }

    // Bytes below are written by hand from the IPMI layouts (response: rqSA, netFn<<2|rqLUN,
    // chk1, rsSA, seq<<2|rsLUN, cmd, CC, data, chk2), not produced by the encoder.

    /// Get Device ID response: seq 5, data per decision 1:
    /// 01 80 00 01 02 09 | 00 00 00 | 4B 49 | 00 00 00 00. chk1 = -(20+1C) = C4,
    /// chk2 = -(82+14+01+00+01+80+00+01+02+09+4B+49) = 48.
    const GET_DEVICE_ID_RESPONSE: [u8; 23] = [
        0x20, 0x1C, 0xC4, 0x82, 0x14, 0x01, 0x00, 0x01, 0x80, 0x00, 0x01, 0x02, 0x09, 0x00, 0x00,
        0x00, 0x4B, 0x49, 0x00, 0x00, 0x00, 0x00, 0x48,
    ];
    /// Get Self Test Results response: data 55 00. chk2 = -(82+14+04+00+55+00) = 11.
    const SELF_TEST_RESPONSE: [u8; 10] =
        [0x20, 0x1C, 0xC4, 0x82, 0x14, 0x04, 0x00, 0x55, 0x00, 0x11];

    #[test]
    fn app_command_responses_are_byte_exact() {
        // (name, command, the exact response frame)
        let cases: [(&str, u8, &[u8]); 2] = [
            ("Get Device ID", 0x01, &GET_DEVICE_ID_RESPONSE),
            ("Get Self Test Results", 0x04, &SELF_TEST_RESPONSE),
        ];
        for (name, cmd, expected) in cases {
            let (frame, _) = run(COMMANDS, NETFN_APP, cmd, &[]);
            assert_eq!(frame, expected, "{name}");
        }
    }

    #[test]
    fn request_data_on_a_no_data_command_is_c7() {
        let (frame, _) = run(COMMANDS, NETFN_APP, 0x01, &[0x00]);
        let resp = ipmb::parse(&frame).unwrap();
        assert_eq!(resp.completion_code, Some(cc::REQ_LEN_INVALID));
        assert!(resp.data.is_empty());
    }

    #[test]
    fn unknown_command_is_c1() {
        // Response: 20 1C C4 82 14 55 C1 chk2 = -(82+14+55+C1) = 54.
        let (frame, _) = run(COMMANDS, NETFN_APP, 0x55, &[]);
        assert_eq!(frame, [0x20, 0x1C, 0xC4, 0x82, 0x14, 0x55, 0xC1, 0x54]);
        // Get SDR Repository Info: a Storage command this IPMC does not implement.
        let (frame, _) = run(COMMANDS, NETFN_STORAGE, 0x20, &[]);
        assert_eq!(
            ipmb::parse(&frame).unwrap().completion_code,
            Some(cc::INVALID_CMD)
        );
    }

    #[test]
    fn table_handlers_and_errors() {
        let table = [
            cmd(NETFN_APP, 0x10, "Echo", "test", echo_handler),
            cmd(NETFN_APP, 0x11, "Fail", "test", failing_handler),
            cmd(NETFN_APP, 0x12, "Long", "test", overlong_handler),
        ];
        let (frame, _) = run(&table, NETFN_APP, 0x10, &[7, 8]);
        let resp = ipmb::parse(&frame).unwrap();
        assert_eq!(
            (resp.completion_code, resp.data),
            (Some(cc::OK), &[7, 8][..])
        );
        let (frame, _) = run(&table, NETFN_APP, 0x11, &[]);
        let resp = ipmb::parse(&frame).unwrap();
        assert_eq!(resp.completion_code, Some(cc::PARAM_OUT_OF_RANGE));
        assert!(resp.data.is_empty());

        let (buf, n) = request_bytes(NETFN_APP, 0x12, &[]);
        let req = ipmb::parse(&buf[..n]).unwrap();
        let mut queue = EgressQueue::new();
        assert_eq!(
            dispatch_with(
                &table,
                &mut context(),
                &req,
                Bus::A,
                0,
                &mut queue,
                &mut NoTrace
            ),
            Err(DispatchError::Frame(IpmbError::TooLong))
        );
        assert!(queue.is_empty());
    }

    #[test]
    fn group_extension_vso_handling() {
        let table = [cmd(NETFN_GROUP_EXT, 0x00, "Echo", "test", echo_handler)];
        // Handler sees the data after the VSO byte; the response carries 03h first.
        let (frame, _) = run(&table, NETFN_GROUP_EXT, 0x00, &[VSO_IDENTIFIER, 0xA1, 0xA2]);
        let resp = ipmb::parse(&frame).unwrap();
        assert_eq!(resp.header.netfn, 0x2D);
        assert_eq!(
            (resp.completion_code, resp.data),
            (Some(cc::OK), &[0x03, 0xA1, 0xA2][..])
        );
        // Another defining body: C1h, echoing that body's byte (IPMI Table 5-1).
        for (data, echoed) in [
            (&[0x05, 0xA1][..], 0x05),
            (&[0x00][..], 0x00),
            (&[0xFE][..], 0xFE),
        ] {
            let (frame, lines) = run(&table, NETFN_GROUP_EXT, 0x00, data);
            let resp = ipmb::parse(&frame).unwrap();
            assert_eq!(
                (resp.completion_code, resp.data),
                (Some(cc::INVALID_CMD), &[echoed][..])
            );
            assert_eq!(lines, ["FN(2Ch) CMD(00h) -> C1h (defining body not VSO)"]);
        }
        // No data at all: no defining body, C7h and no data (IPMI Table 5-2, section 5.3.1).
        let (frame, lines) = run(&table, NETFN_GROUP_EXT, 0x00, &[]);
        let resp = ipmb::parse(&frame).unwrap();
        assert_eq!(
            (resp.completion_code, resp.data),
            (Some(cc::REQ_LEN_INVALID), &[][..])
        );
        assert_eq!(lines, ["FN(2Ch) CMD(00h) -> C7h (no defining body byte)"]);
        // Unknown command with a good VSO byte.
        let (frame, _) = run(&table, NETFN_GROUP_EXT, 0x77, &[VSO_IDENTIFIER]);
        let resp = ipmb::parse(&frame).unwrap();
        assert_eq!(
            (resp.completion_code, resp.data),
            (Some(cc::INVALID_CMD), &[0x03][..])
        );
    }

    #[test]
    fn short_output_buffer_runs_no_handler() {
        let table = [cmd(NETFN_APP, 0x20, "Record", "test", recording_handler)];
        let (buf, n) = request_bytes(NETFN_APP, 0x20, &[]);
        let req = ipmb::parse(&buf[..n]).unwrap();
        let mut short = [0u8; ipmb::IPMB_MAX_LEN - 1];
        assert_eq!(
            build_response_with(&table, &mut context(), &req, &mut short),
            Err(IpmbError::OutBufferTooSmall)
        );
        assert!(
            !RAN.load(Ordering::Relaxed),
            "handler ran with a short buffer"
        );
        let mut full = [0u8; ipmb::IPMB_MAX_LEN];
        build_response_with(&table, &mut context(), &req, &mut full).unwrap();
        assert!(RAN.load(Ordering::Relaxed));
    }

    #[test]
    fn trace_lines_for_hit_and_miss() {
        let (_, lines) = run(COMMANDS, NETFN_APP, 0x01, &[]);
        assert_eq!(
            lines,
            ["FN(06h) CMD(01h) Get Device ID -> CC 00h, 15 bytes"]
        );
        let (_, lines) = run(COMMANDS, NETFN_APP, 0x55, &[]);
        assert_eq!(lines, ["FN(06h) CMD(55h) -> C1h (no handler)"]);
    }

    #[test]
    fn full_queue_reports_and_counts() {
        let (buf, n) = request_bytes(NETFN_APP, 0x01, &[]);
        let req = ipmb::parse(&buf[..n]).unwrap();
        let mut queue = EgressQueue::new();
        for _ in 0..crate::queue::TXQ_DEPTH {
            dispatch(&mut context(), &req, Bus::A, 0, &mut queue, &mut NoTrace).unwrap();
        }
        assert_eq!(
            dispatch(&mut context(), &req, Bus::A, 0, &mut queue, &mut NoTrace),
            Err(DispatchError::Queue(QueueError::Full))
        );
        assert_eq!(queue.dropped(), 1);
    }

    static LUN_RAN: AtomicBool = AtomicBool::new(false);
    fn lun_handler(_c: &mut Context, _r: &Frame, _o: &mut [u8]) -> Result<usize, u8> {
        LUN_RAN.store(true, Ordering::Relaxed);
        Ok(0)
    }

    /// Request bytes with destination LUN `lun` (rsLUN in byte 1) and request `data`, checksums
    /// rebuilt.
    fn request_to_lun(
        netfn: u8,
        cmd: u8,
        lun: u8,
        data: &[u8],
    ) -> ([u8; ipmb::IPMB_MAX_LEN], usize) {
        let header = ipmb::Header {
            dest: 0x82,
            netfn,
            dest_lun: lun,
            src: 0x20,
            seq: 5,
            src_lun: 0,
            cmd,
        };
        let mut buf = [0u8; ipmb::IPMB_MAX_LEN];
        let n = ipmb::encode_request(&header, data, &mut buf).unwrap();
        (buf, n)
    }

    #[test]
    fn nonzero_lun_is_c2_before_any_handler() {
        let table = [cmd(NETFN_APP, 0x30, "Lun", "test", lun_handler)];
        for lun in 1..=3u8 {
            let (buf, n) = request_to_lun(NETFN_APP, 0x30, lun, &[]);
            let req = ipmb::parse(&buf[..n]).unwrap();
            let mut queue = EgressQueue::new();
            let mut trace = RecordingTrace::default();
            dispatch_with(
                &table,
                &mut context(),
                &req,
                Bus::A,
                0,
                &mut queue,
                &mut trace,
            )
            .unwrap();
            let frame = queue.pop().unwrap();
            let resp = ipmb::parse(frame.as_bytes()).unwrap();
            assert_eq!(resp.completion_code, Some(cc::INVALID_FOR_LUN));
            assert!(resp.data.is_empty());
            assert_eq!(
                trace.lines,
                [format!("FN(06h) CMD(30h) -> C2h (LUN {lun} not supported)")]
            );
        }
        assert!(
            !LUN_RAN.load(Ordering::Relaxed),
            "handler ran for a LUN it does not serve"
        );
        // The same request on LUN 0 reaches the handler.
        let (buf, n) = request_to_lun(NETFN_APP, 0x30, 0, &[]);
        let req = ipmb::parse(&buf[..n]).unwrap();
        let mut queue = EgressQueue::new();
        dispatch_with(
            &table,
            &mut context(),
            &req,
            Bus::A,
            0,
            &mut queue,
            &mut NoTrace,
        )
        .unwrap();
        assert!(LUN_RAN.load(Ordering::Relaxed));
        let frame = queue.pop().unwrap();
        let resp = ipmb::parse(frame.as_bytes()).unwrap();
        assert_eq!(resp.completion_code, Some(cc::OK));
    }

    #[test]
    fn nonzero_lun_on_group_extension_is_answered_c2h_first() {
        // C2h takes precedence over a missing (C7h) or foreign (C1h) defining body and over the
        // command; the response echoes the request's defining-body byte when it has one.
        let cases: [(&[u8], &[u8]); 4] = [
            (&[], &[]),
            (&[VSO_IDENTIFIER], &[VSO_IDENTIFIER]),
            (&[VSO_IDENTIFIER, 0xA1], &[VSO_IDENTIFIER]),
            (&[0x05, 0xA1], &[0x05]),
        ];
        for (data, echoed) in cases {
            let (buf, n) = request_to_lun(NETFN_GROUP_EXT, 0x00, 2, data);
            let req = ipmb::parse(&buf[..n]).unwrap();
            let mut queue = EgressQueue::new();
            dispatch_with(
                &[],
                &mut context(),
                &req,
                Bus::A,
                0,
                &mut queue,
                &mut NoTrace,
            )
            .unwrap();
            let frame = queue.pop().unwrap();
            let resp = ipmb::parse(frame.as_bytes()).unwrap();
            assert_eq!(
                (resp.completion_code, resp.data),
                (Some(cc::INVALID_FOR_LUN), echoed),
                "request data {data:02X?}"
            );
        }
    }

    static FULL_RAN: AtomicBool = AtomicBool::new(false);
    fn full_handler(_c: &mut Context, _r: &Frame, _o: &mut [u8]) -> Result<usize, u8> {
        FULL_RAN.store(true, Ordering::Relaxed);
        Ok(0)
    }

    #[test]
    fn full_queue_runs_no_handler() {
        let table = [cmd(NETFN_APP, 0x40, "Full", "test", full_handler)];
        let (buf, n) = request_bytes(NETFN_APP, 0x40, &[]);
        let req = ipmb::parse(&buf[..n]).unwrap();
        let mut queue = EgressQueue::new();
        for _ in 0..crate::queue::TXQ_DEPTH {
            queue.push(&[0u8; 7], Bus::A, 0).unwrap();
        }
        assert_eq!(
            dispatch_with(
                &table,
                &mut context(),
                &req,
                Bus::A,
                0,
                &mut queue,
                &mut NoTrace
            ),
            Err(DispatchError::Queue(QueueError::Full))
        );
        assert!(
            !FULL_RAN.load(Ordering::Relaxed),
            "handler ran with a full queue"
        );
        assert_eq!(queue.dropped(), 1);
        // One free slot: the handler runs and the response is queued.
        queue.pop().unwrap();
        dispatch_with(
            &table,
            &mut context(),
            &req,
            Bus::A,
            0,
            &mut queue,
            &mut NoTrace,
        )
        .unwrap();
        assert!(FULL_RAN.load(Ordering::Relaxed));
        assert_eq!(queue.dropped(), 1);
    }

    #[test]
    fn update_sensors_stores_the_snapshot_and_steps_the_fru_state() {
        use crate::fru_state::State;
        let mut c = context();
        assert_eq!(c.fru_state.state(), State::M0);
        let mut on = SensorSnapshot::EMPTY;
        on.power = PowerState::On;
        assert!(c.update_sensors(1000, &on, &mut NoTrace).is_some());
        assert_eq!(
            (c.fru_state.state(), c.sensors.power),
            (State::M1, PowerState::On)
        );
        assert!(c.update_sensors(2000, &on, &mut NoTrace).is_some());
        assert_eq!(c.fru_state.state(), State::M4);
        // Power not On (off, requested, fault): payload power is not valid.
        assert!(c
            .update_sensors(3000, &SensorSnapshot::EMPTY, &mut NoTrace)
            .is_some());
        assert_eq!(c.fru_state.state(), State::M6);
    }

    #[test]
    fn update_sensors_traces_each_fru_state_change_once() {
        let mut c = context();
        let mut on = SensorSnapshot::EMPTY;
        on.power = PowerState::On;
        let mut trace = RecordingTrace::default();
        c.update_sensors(1000, &on, &mut trace);
        c.update_sensors(2000, &on, &mut trace);
        c.update_sensors(3000, &on, &mut trace); // no change, no line
        assert_eq!(
            trace.lines,
            [
                "FRU state: M0 IPMC Inactive -> M1 FRU Inactive",
                "FRU state: M1 FRU Inactive -> M4 FRU Active"
            ]
        );
    }

    #[test]
    fn a_bad_address_strap_keeps_the_fru_in_m0() {
        // Raw word 7Fh: GA 0 (invalid) with even parity.
        let mut c = Context::new(addr::decode(0x7F));
        assert!(!c.initial_checks().passed());
        let mut on = SensorSnapshot::EMPTY;
        on.power = PowerState::On;
        assert!(c.update_sensors(1000, &on, &mut NoTrace).is_none());
        assert_eq!(c.fru_state.state(), crate::fru_state::State::M0);
    }

    #[test]
    fn response_is_rejected_as_request() {
        let header = ipmb::Header {
            dest: 0x20,
            netfn: 7,
            dest_lun: 0,
            src: 0x82,
            seq: 3,
            src_lun: 0,
            cmd: 1,
        };
        let mut buf = [0u8; ipmb::IPMB_MAX_LEN];
        let n = ipmb::encode_response(&header, cc::OK, &[], &mut buf).unwrap();
        let req = ipmb::parse(&buf[..n]).unwrap();
        let mut queue = EgressQueue::new();
        assert_eq!(
            dispatch(&mut context(), &req, Bus::A, 0, &mut queue, &mut NoTrace),
            Err(DispatchError::Frame(IpmbError::NotARequest))
        );
    }
}

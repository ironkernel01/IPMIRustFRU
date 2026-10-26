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
use crate::event_enable::EventEnables;
use crate::ext::{Extension, NoExtension};
use crate::fru_state::{FruMachine, InitialChecks, Inputs, Transition};
use crate::ipmb::{self, Frame, IpmbError, NETFN_GROUP_EXT};
use crate::link::{BusPair, LinkConfig, LinkView, PendingIpmbState};
use crate::queue::{Bus, EgressQueue, QueueError};
use crate::sdr::SdrRepository;
use crate::selftest::{self, SelfTest};
use crate::sensor::{
    self, EncodeError, PayloadMode, PowerState, SensorSnapshot, SENSOR_FRU_HEALTH,
    SENSOR_FRU_STATE, SENSOR_FRU_TEMPERATURE, SENSOR_FRU_VOLTAGE, SENSOR_PAYLOAD_MODE,
    SENSOR_PAYLOAD_TEST_RESULTS, SENSOR_PAYLOAD_TEST_STATUS, SENSOR_SYSTEM_IPMB_LINK,
};
use crate::tier;
use crate::trace::Trace;
use ipmc_fru::{FieldError, FruField, FruStore};

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

/// ResetRequest -- a restart a handler asks for through `Context::pending_reset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetRequest {
    /// Cold reset: the processor restarts as after power-up (IPMI v2.0 Cold Reset).
    Cold,
    /// Warm reset: the IPMC re-initialises its volatile link state without a processor reset
    /// (IPMI v2.0 Warm Reset): a new Link in the power-up bus state with the same configuration
    /// and address, the request sequence state carried over as for a Cold Reset, the `pending_*`
    /// hand-offs cleared and `Extension::warm_reset` called. Kept: the FRU store, the SDR
    /// repository, the self test result, the FRU state, the sensors, the payload power and
    /// PWR_EN, the non-volatile settings, `payload_mode` and `event_enables` (the event enables
    /// survive, IPMI v2.0 20.3). The IPMB state does not: IPMB-B returns to isolated and both buses
    /// to 100 kHz (VITA 46.11 Rules 4.1.9-2 and 4.1.1-4.3). `Context::warm_reset` is the one function that clears
    /// the Context part.
    Warm,
}

/// PowerRequest -- a payload power change a handler asks for through `Context::pending_power`
/// (the Tier-3 Set FRU Activation handler, Rule 4.1.2-10.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerRequest {
    /// Switch payload power on (the owner asks `FruMachine::check_power_on` first).
    On,
    /// Switch payload power off.
    Off,
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
/// - `event_enables`: the per-sensor event enables; written only by the Tier-3 Set Sensor Event
///   Enable handler (extension); read by Get Sensor Reading (status byte bit 7) and by the
///   Tier-2/3 event generator before it sends. The SDR builder's masks come from the same table
///   (`sdr::event_masks`).
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
/// - `pending_power`: a handler's request to switch payload power on or off (`PowerRequest`). The
///   owner of the payload power (the firmware main loop, the emulator) takes it after the handler
///   returns and applies it through `Monitor::power_on` / `power_off`; for On it asks
///   `FruMachine::check_power_on` first (owner decision 9: one check for every power-on path)
///   and traces a refusal. No Tier-1 or Tier-2 handler sets it.
/// - `deactivation_requested`: a Tier-3 handler's standing request that the FRU deactivates
///   (Rules 4.1.5.4.3-2, -6); `update_sensors` feeds it to the FRU state machine (M4 to M5) at
///   Tier-3 only (`tier::activation_control`). The handler writes it; nothing else does.
/// - `pending_payload_reset`: a handler's request for a pulse on the payload reset output
///   (`PAYLOAD_RST_OUT_N`, Rule 4.1.2-9); the owner of the pin (the firmware main loop, the
///   emulator) takes it and pulses the output for IPMC_PAYLOAD_RESET_PULSE_MS.
/// - `payload_mode`: the state of the Payload Mode sensor (sensor 7, Tier-3); the Set Payload Mode
///   handler is its only writer, `read_sensor` reads it; power-up mode 0, transition complete.
/// - the non-volatile settings byte (bit 0 Default-Activation-Locked, bit 1
///   Commanded-Deactivation-Ignored, Rule 4.1.2-10.2) lives in `fru` (`FruStore::nv_settings`)
///   so the flash log saves it with the image on the same dirty mark; `nv_settings` and
///   `set_nv_settings` below are its accessors, the Tier-3 Set FRU State Policy Bits handler the
///   only writer and the firmware boot (`set_fru`) the loader. No Tier-1 or Tier-2 handler
///   writes it.
/// - `pending_reset`: a handler's request that the processor restarts (the Tier-2/3 Cold Reset
///   handler) or that the IPMC re-initialises its link state (Warm Reset, `ResetRequest::Warm`):
///   the handler validates the request and sets it. The owner of the processor (the firmware
///   main loop, the emulator) applies it after the response has left the egress queue, or
///   `IPMC_RESET_WAIT_MS` after it took the request (`ResetGate`, the same for both). A Warm
///   Reset rebuilds the Link and calls `Context::warm_reset`. No Tier-1 handler sets it.
pub struct Context {
    /// Resolved geographic address and NVMRO state.
    pub addr: AddrInfo,
    /// Latest sensor snapshot (`SensorSnapshot::EMPTY` before the first sample).
    pub sensors: SensorSnapshot,
    /// RAM FRU store of FRU 0.
    pub fru: FruStore,
    /// Device SDR repository and reservation state.
    pub sdr: SdrRepository,
    /// Per-sensor event enables (power-up values of this build's Tier).
    pub event_enables: EventEnables,
    /// Result of the latest self test (IPMI v2.0 Table 20-5).
    pub self_test: SelfTest,
    /// Operational State of FRU 0.
    pub fru_state: FruMachine,
    /// Read-only view of the link layer for the current dispatch.
    pub link: LinkView,
    /// Set IPMB State change for the link layer to apply after the handler returns.
    pub pending_ipmb: Option<PendingIpmbState>,
    /// Processor restart for the owner of the processor to apply after the response left.
    pub pending_reset: Option<ResetRequest>,
    /// Payload power change for the owner of the payload power to apply after the handler returns.
    pub pending_power: Option<PowerRequest>,
    /// A deactivation is requested (Tier-3); fed to the FRU state machine by `update_sensors`.
    pub deactivation_requested: bool,
    /// A payload reset pulse for the owner of the reset output to start.
    pub pending_payload_reset: bool,
    /// State of the Payload Mode sensor.
    pub payload_mode: PayloadMode,
}

impl Context {
    /// new -- the start-up context for the resolved address `addr`: the FRU image and the SDR
    /// repository built (owner = this controller's IPMB address), the startup self test run on
    /// them, no sensor sample yet, FRU state M0 and the link view of a fresh link with the
    /// defs.rs configuration.
    pub fn new(addr: AddrInfo) -> Context {
        let fru = FruStore::new();
        let sdr = SdrRepository::new(addr.ipmb_address);
        let self_test = selftest::run(fru.bytes(), &sdr);
        Context {
            addr,
            sensors: SensorSnapshot::EMPTY,
            fru,
            sdr,
            event_enables: EventEnables::power_up(),
            self_test,
            fru_state: FruMachine::new(),
            link: LinkView::power_up(LinkConfig::from_defs(), addr.usable()),
            pending_ipmb: None,
            pending_reset: None,
            pending_power: None,
            deactivation_requested: false,
            pending_payload_reset: false,
            payload_mode: PayloadMode::POWER_UP,
        }
    }

    /// warm_reset -- the Context part of a Warm Reset (`ResetRequest::Warm`): `link` is the view
    /// of the new Link the owner built (`Link::restarted`); every `pending_*` hand-off is cleared,
    /// the Warm Reset request itself included. Everything else is kept (see `ResetRequest::Warm`).
    pub fn warm_reset(&mut self, link: LinkView) {
        self.link = link;
        self.pending_ipmb = None;
        self.pending_power = None;
        self.pending_payload_reset = false;
        self.pending_reset = None;
    }

    /// nv_settings -- the Tier-3 non-volatile settings byte (bit 0 Default-Activation-Locked, bit
    /// 1 Commanded-Deactivation-Ignored); 00h without a stored record.
    pub const fn nv_settings(&self) -> u8 {
        self.fru.nv_settings()
    }

    /// set_nv_settings -- changes the settings byte; a change marks the FRU store dirty, so the
    /// flash log saves it as it saves a Write FRU Data. The caller checks NVMRO first.
    pub fn set_nv_settings(&mut self, settings: u8) {
        self.fru.set_nv_settings(settings);
    }

    /// set_fru -- replaces the FRU store (the image loaded from flash at boot, or the built image
    /// after FRU RESTORE) and reruns the self test on it, so Get Self Test Results and the FRU
    /// initial checks (Rule 4.1.5.4.1-9) judge the image now in use: a corrupt stored image
    /// gives 57h with the FRU bit and keeps the FRU in M0 (owner decision 8).
    pub fn set_fru(&mut self, fru: FruStore) {
        self.fru = fru;
        self.self_test = selftest::run(self.fru.bytes(), &self.sdr);
    }

    /// set_unit_field -- the application port's write of one per-unit FRU field (`SET` of
    /// ipmc-app): refused with `FieldError::Nvmro` while NVMRO is asserted (VITA 46.11 Rule 8-3,
    /// as Write FRU Data is), else `FruStore::set_unit_field` and the self test on the new image,
    /// as after a Write FRU Data. The store is marked dirty, so the flash log saves the image.
    pub fn set_unit_field(&mut self, field: FruField, value: &str) -> Result<(), FieldError> {
        if self.addr.nvmro_asserted {
            return Err(FieldError::Nvmro);
        }
        self.fru.set_unit_field(field, value)?;
        self.self_test = selftest::run(self.fru.bytes(), &self.sdr);
        Ok(())
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
            deactivation_requested: self.deactivation_requested
                && tier::activation_control(ipmc_defs::IPMC_TIER),
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
    /// - 6 Payload Test Status: not in progress (Table 10.2.7-1);
    /// - 7 Payload Mode (Tier-3 only, Table 10.2.8-1) from `payload_mode`; four bytes.
    ///
    /// Status byte bit 7 (event messages enabled) is the sensor's `event_enables` flag.
    ///
    /// Returns the byte count, Err(NotPresent) for a number the build's Tier does not have and
    /// Err(OutputTooSmall) when `out` cannot hold a reading (`sensor::READING_LEN_MAX` holds
    /// every one).
    pub fn read_sensor(
        &self,
        number: u8,
        buses: &BusPair,
        out: &mut [u8],
    ) -> Result<usize, SensorReadError> {
        self.read_sensor_at(ipmc_defs::IPMC_TIER, number, buses, out)
    }

    /// read_sensor_at -- `read_sensor` as a Tier-`tier` IPMC (`read_sensor` passes the build's
    /// Tier): sensor 7 exists only where `tier::payload_mode_sensor`.
    pub fn read_sensor_at(
        &self,
        tier: u32,
        number: u8,
        buses: &BusPair,
        out: &mut [u8],
    ) -> Result<usize, SensorReadError> {
        let encoded = match number {
            SENSOR_PAYLOAD_MODE if tier::payload_mode_sensor(tier) => {
                sensor::encode_payload_mode(&self.payload_mode, self.addr.nvmro_asserted, out)
            }
            SENSOR_FRU_STATE => sensor::encode_fru_state(self.fru_state.state().mask(), out),
            SENSOR_SYSTEM_IPMB_LINK => sensor::encode_link(&buses.sensor_data(), out),
            SENSOR_FRU_HEALTH | SENSOR_FRU_VOLTAGE | SENSOR_FRU_TEMPERATURE => {
                sensor::encode_sensor_reading(number, &self.sensors, out)
            }
            SENSOR_PAYLOAD_TEST_RESULTS => sensor::encode_payload_test_results(out),
            SENSOR_PAYLOAD_TEST_STATUS => sensor::encode_payload_test_status(false, out),
            _ => return Err(SensorReadError::NotPresent),
        };
        let len = encoded.map_err(|error| match error {
            EncodeError::BufferTooShort => SensorReadError::OutputTooSmall,
            EncodeError::UnsupportedSensor => SensorReadError::NotPresent,
        })?;
        let enabled = self.event_enables.get(number).is_some_and(|e| e.enabled);
        sensor::apply_event_enable(out, enabled);
        Ok(len)
    }
}

/// SensorReadError -- why `Context::read_sensor` produced no reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorReadError {
    /// The sensor number is not one of the mandatory sensors of the Tier.
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

/// run_command -- looks up the row and runs its handler into `data`; on a table miss the
/// extension gets the request (Tier-1 rows win over it). One result path for both.
/// Returns (completion code, outcome, data length); TooLong when the handler reports more
/// bytes than `data` holds.
fn run_command(
    table: &[Command],
    ext: &mut dyn Extension,
    ctx: &mut Context,
    req: &Frame,
    data: &mut [u8],
) -> Result<(u8, Outcome, usize), IpmbError> {
    let (name, result) = match commands::find_in(table, req.header.netfn, req.header.cmd) {
        Some(row) => (row.name, (row.handler)(ctx, req, data)),
        None => match ext.command(ctx, req, data) {
            Some(answer) => answer,
            None => return Ok((cc::INVALID_CMD, Outcome::NoHandler, 0)),
        },
    };
    let outcome = Outcome::Handled(name);
    match result {
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
    ext: &mut dyn Extension,
    ctx: &mut Context,
    req: &Frame,
    data: &mut [u8],
) -> Result<(u8, Outcome, usize), IpmbError> {
    if req.header.netfn != NETFN_GROUP_EXT {
        if let Some((code, outcome)) = unsupported_lun(req) {
            return Ok((code, outcome, 0));
        }
        return run_command(table, ext, ctx, req, data);
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
    let (code, outcome, len) = run_command(table, ext, ctx, &view, handler_out)?;
    Ok((code, outcome, len.checked_add(1).ok_or(IpmbError::TooLong)?))
}

/// build_response_with -- builds the complete response frame for `req` into `out_frame`
/// using `table`. The single frame builder behind `dispatch` and the tests.
/// 1. Builds the response header (NotARequest when `req` is a response).
/// 2. Checks `out_frame` holds a whole frame (OutBufferTooSmall) BEFORE any handler runs, so
///    a handler's side effects never happen for a response that cannot be delivered.
/// 3. Runs the request (`ext` answers a (netFn, cmd) that `table` has no row for); encodes the
///    response.
pub fn build_response_with(
    table: &[Command],
    ext: &mut dyn Extension,
    ctx: &mut Context,
    req: &Frame,
    out_frame: &mut [u8],
) -> Result<Built, IpmbError> {
    let header = ipmb::response_header_for(req)?;
    if out_frame.len() < ipmb::IPMB_MAX_LEN {
        return Err(IpmbError::OutBufferTooSmall);
    }
    let mut data = [0u8; ipmb::IPMB_RESPONSE_DATA_MAX];
    let (completion_code, outcome, data_len) = run_request(table, ext, ctx, req, &mut data)?;
    let payload = data.get(..data_len).ok_or(IpmbError::TooLong)?;
    let len = ipmb::encode_response(&header, completion_code, payload, out_frame)?;
    Ok(Built {
        len,
        completion_code,
        data_len,
        outcome,
    })
}

/// trace_request -- one PIC32-style line for a dispatched request, when IPMC_IPMI_TRACE is on.
/// `FN(06h) CMD(01h) Get Device ID -> CC 00h, 15 bytes`
/// `FN(06h) CMD(55h) -> C1h (no handler)`
fn trace_request(trace: &mut dyn Trace, req: &Frame, built: &Built) {
    if !ipmc_defs::IPMC_IPMI_TRACE {
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

/// dispatch_with -- answers `req` using `table` and, on a table miss, `ext`: builds the response,
/// traces the request and pushes the frame onto `queue` with the bus the request arrived on and
/// its arrival time (`arrival_ms`, the request STOP); the link layer chooses the transmit bus
/// later.
/// The queue is checked for a free slot BEFORE any handler runs (a handler has side effects;
/// a response that cannot be queued must not cause them): a full queue runs nothing, counts a
/// drop and returns Err(Queue(Full)).
/// Err(Frame(..)) = nothing built (no handler ran for OutBufferTooSmall/NotARequest);
/// Err(Queue(..)) = not queued.
// Eight arguments: table, extension, context and request are the
// four things dispatch needs, and the other four are the queue entry; a parameter struct would
// only rename them.
#[allow(clippy::too_many_arguments)]
pub fn dispatch_with(
    table: &[Command],
    ext: &mut dyn Extension,
    ctx: &mut Context,
    req: &Frame,
    arrival: Bus,
    arrival_ms: u32,
    queue: &mut EgressQueue,
    trace: &mut dyn Trace,
) -> Result<(), DispatchError> {
    queue.reserve().map_err(DispatchError::Queue)?;
    let mut frame = [0u8; ipmb::IPMB_MAX_LEN];
    let built =
        build_response_with(table, ext, ctx, req, &mut frame).map_err(DispatchError::Frame)?;
    trace_request(trace, req, &built);
    let bytes = frame
        .get(..built.len)
        .ok_or(DispatchError::Frame(IpmbError::TooLong))?;
    queue
        .push(bytes, arrival, arrival_ms)
        .map_err(DispatchError::Queue)
}

/// dispatch -- `dispatch_with` over the COMMANDS table, with no extension (the Tier-1 build).
pub fn dispatch(
    ctx: &mut Context,
    req: &Frame,
    arrival: Bus,
    arrival_ms: u32,
    queue: &mut EgressQueue,
    trace: &mut dyn Trace,
) -> Result<(), DispatchError> {
    dispatch_with(
        COMMANDS,
        &mut NoExtension,
        ctx,
        req,
        arrival,
        arrival_ms,
        queue,
        trace,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addr;
    use crate::commands::cmd;
    use crate::ext::NoExtension;
    use crate::ipmb::{NETFN_APP, NETFN_STORAGE};
    use crate::trace::{NoTrace, RecordingTrace};
    use core::sync::atomic::{AtomicBool, Ordering};

    fn context() -> Context {
        Context::new(addr::resolve(0x7F))
    }

    #[test]
    fn set_unit_field_edits_the_image_reruns_the_self_test_and_honours_nvmro() {
        let mut ctx = context();
        assert!(!ctx.fru.is_dirty());
        ctx.set_unit_field(FruField::BoardSerial, "00:11:22:33:44:55")
            .unwrap();
        assert!(ctx.fru.is_dirty());
        assert!(ctx.self_test.passed());
        assert_eq!(
            ipmc_fru::unit_field(ctx.fru.bytes(), FruField::BoardSerial),
            Ok(ipmc_fru::FieldValue::Text("00:11:22:33:44:55"))
        );
        // NVMRO asserted: refused, the image stays.
        let before = *ctx.fru.image();
        ctx.addr.nvmro_asserted = true;
        assert_eq!(
            ctx.set_unit_field(FruField::AssetTag, "abc"),
            Err(FieldError::Nvmro)
        );
        assert_eq!(ctx.fru.image(), &before);
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
            &mut NoExtension,
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

    /// Reports the byte count named by its first request data byte, writing nothing; shows where
    /// the handler output limit lies.
    fn length_handler(_c: &mut Context, r: &Frame, _o: &mut [u8]) -> Result<usize, u8> {
        Ok(usize::from(r.data[0]))
    }

    static RAN: AtomicBool = AtomicBool::new(false);
    fn recording_handler(_c: &mut Context, _r: &Frame, _o: &mut [u8]) -> Result<usize, u8> {
        RAN.store(true, Ordering::Relaxed);
        Ok(0)
    }

    // Bytes below are written by hand from the IPMI layouts (response: rqSA, netFn<<2|rqLUN,
    // chk1, rsSA, seq<<2|rsLUN, cmd, CC, data, chk2), not produced by the encoder.

    /// Get Device ID response: seq 5, data per decision 1:
    /// 01 80 00 01 02 AS | 00 00 00 | 4B 49 | 00 00 00 00 with the Additional Device Support byte
    /// AS = 09h at Tier-1, 29h at Tier-2 (Event Generator), 2Dh at Tier-3 (and SEL Device).
    /// chk1 = -(20+1C) = C4, chk2 = -(82+14+01+00+01+80+00+01+02+AS+4B+49) = 48h, 28h, 24h.
    fn get_device_id_response() -> [u8; 23] {
        let (support, chk2) =
            [(0x09, 0x48), (0x29, 0x28), (0x2D, 0x24)][ipmc_defs::IPMC_TIER as usize - 1];
        [
            0x20, 0x1C, 0xC4, 0x82, 0x14, 0x01, 0x00, 0x01, 0x80, 0x00, 0x01, 0x02, support, 0x00,
            0x00, 0x00, 0x4B, 0x49, 0x00, 0x00, 0x00, 0x00, chk2,
        ]
    }
    /// Get Self Test Results response: data 55 00. chk2 = -(82+14+04+00+55+00) = 11.
    const SELF_TEST_RESPONSE: [u8; 10] =
        [0x20, 0x1C, 0xC4, 0x82, 0x14, 0x04, 0x00, 0x55, 0x00, 0x11];

    #[test]
    fn app_command_responses_are_byte_exact() {
        // (name, command, the exact response frame)
        let cases: [(&str, u8, &[u8]); 2] = [
            ("Get Device ID", 0x01, &get_device_id_response()),
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
        let table: [Command; 3] = [
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
                &mut NoExtension,
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
        let table: [Command; 1] = [cmd(NETFN_GROUP_EXT, 0x00, "Echo", "test", echo_handler)];
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
    fn group_extension_handler_length_limit_counts_the_echoed_defining_body() {
        let table: [Command; 1] = [cmd(NETFN_GROUP_EXT, 0x00, "Length", "test", length_handler)];
        // The handler output holds one byte less than the response data: the echoed defining
        // body byte takes the first.
        let fits = ipmb::IPMB_RESPONSE_DATA_MAX - 1;
        // (handler result length, whether the response is built)
        let cases = [
            (0, true),
            (1, true),
            (fits - 1, true),
            (fits, true),
            (fits + 1, false),
            (fits + 2, false),
        ];
        for (len, built) in cases {
            let request_data = [VSO_IDENTIFIER, u8::try_from(len).unwrap()];
            let (buf, n) = request_bytes(NETFN_GROUP_EXT, 0x00, &request_data);
            let req = ipmb::parse(&buf[..n]).unwrap();
            let mut out = [0u8; ipmb::IPMB_MAX_LEN];
            let result =
                build_response_with(&table, &mut NoExtension, &mut context(), &req, &mut out);
            if built {
                let b = result.unwrap_or_else(|e| panic!("handler length {len}: {e:?}"));
                assert_eq!(b.data_len, len + 1, "handler length {len}");
                assert_eq!(b.completion_code, cc::OK);
                let resp = ipmb::parse(&out[..b.len]).unwrap();
                assert_eq!(resp.data.len(), len + 1, "handler length {len}");
                assert_eq!(resp.data[0], VSO_IDENTIFIER);
            } else {
                assert_eq!(result, Err(IpmbError::TooLong), "handler length {len}");
            }
        }
        // The largest fitting response fills the frame exactly.
        let request_data = [VSO_IDENTIFIER, u8::try_from(fits).unwrap()];
        let (buf, n) = request_bytes(NETFN_GROUP_EXT, 0x00, &request_data);
        let req = ipmb::parse(&buf[..n]).unwrap();
        let mut out = [0u8; ipmb::IPMB_MAX_LEN];
        let b =
            build_response_with(&table, &mut NoExtension, &mut context(), &req, &mut out).unwrap();
        assert_eq!(b.len, ipmb::IPMB_MAX_LEN);
    }

    #[test]
    fn short_output_buffer_runs_no_handler() {
        let table: [Command; 1] = [cmd(NETFN_APP, 0x20, "Record", "test", recording_handler)];
        let (buf, n) = request_bytes(NETFN_APP, 0x20, &[]);
        let req = ipmb::parse(&buf[..n]).unwrap();
        let mut short = [0u8; ipmb::IPMB_MAX_LEN - 1];
        assert_eq!(
            build_response_with(&table, &mut NoExtension, &mut context(), &req, &mut short),
            Err(IpmbError::OutBufferTooSmall)
        );
        assert!(
            !RAN.load(Ordering::Relaxed),
            "handler ran with a short buffer"
        );
        let mut full = [0u8; ipmb::IPMB_MAX_LEN];
        build_response_with(&table, &mut NoExtension, &mut context(), &req, &mut full).unwrap();
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
        let table: [Command; 1] = [cmd(NETFN_APP, 0x30, "Lun", "test", lun_handler)];
        for lun in 1..=3u8 {
            let (buf, n) = request_to_lun(NETFN_APP, 0x30, lun, &[]);
            let req = ipmb::parse(&buf[..n]).unwrap();
            let mut queue = EgressQueue::new();
            let mut trace = RecordingTrace::default();
            dispatch_with(
                &table,
                &mut NoExtension,
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
            &mut NoExtension,
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
                &mut NoExtension,
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
        let table: [Command; 1] = [cmd(NETFN_APP, 0x40, "Full", "test", full_handler)];
        let (buf, n) = request_bytes(NETFN_APP, 0x40, &[]);
        let req = ipmb::parse(&buf[..n]).unwrap();
        let mut queue = EgressQueue::new();
        for _ in 0..crate::queue::TXQ_DEPTH {
            queue.push(&[0u8; 7], Bus::A, 0).unwrap();
        }
        assert_eq!(
            dispatch_with(
                &table,
                &mut NoExtension,
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
            &mut NoExtension,
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

    // ---- the extension hook ----

    /// ExtAnswer -- test extension: answers one (netFn, cmd) with `result`, writing `fill` into
    /// the output, and records the request data and whether it was asked.
    struct ExtAnswer {
        netfn: u8,
        cmd: u8,
        result: Result<usize, u8>,
        asked: bool,
        seen: Vec<u8>,
    }

    impl ExtAnswer {
        const FILL: u8 = 0xAB;

        fn new(netfn: u8, cmd: u8, result: Result<usize, u8>) -> ExtAnswer {
            ExtAnswer {
                netfn,
                cmd,
                result,
                asked: false,
                seen: Vec::new(),
            }
        }
    }

    impl Extension for ExtAnswer {
        fn command(
            &mut self,
            _ctx: &mut Context,
            req: &Frame,
            out: &mut [u8],
        ) -> Option<(&'static str, Result<usize, u8>)> {
            self.asked = true;
            if (req.header.netfn, req.header.cmd) != (self.netfn, self.cmd) {
                return None;
            }
            self.seen = req.data.to_vec();
            if let Ok(len) = self.result {
                out.iter_mut().take(len).for_each(|b| *b = ExtAnswer::FILL);
            }
            Some(("Ext Test", self.result))
        }
    }

    /// ext_response -- builds the response to a request on `lun` through `table` and `ext`;
    /// Ok((built, response frame bytes)).
    fn ext_response(
        table: &[Command],
        ext: &mut dyn Extension,
        (netfn, cmd, lun): (u8, u8, u8),
        data: &[u8],
    ) -> Result<(Built, Vec<u8>), IpmbError> {
        let (buf, n) = request_to_lun(netfn, cmd, lun, data);
        let req = ipmb::parse(&buf[..n]).unwrap();
        let mut out = [0u8; ipmb::IPMB_MAX_LEN];
        let built = build_response_with(table, ext, &mut context(), &req, &mut out)?;
        Ok((built, out[..built.len].to_vec()))
    }

    #[test]
    fn extension_answers_only_on_a_table_miss() {
        let table: [Command; 1] = [cmd(NETFN_APP, 0x10, "Echo", "test", echo_handler)];
        // The extension claims the table's own command too: the table row wins.
        let mut ext = ExtAnswer::new(NETFN_APP, 0x10, Ok(1));
        let (built, frame) = ext_response(&table, &mut ext, (NETFN_APP, 0x10, 0), &[7, 8]).unwrap();
        assert_eq!(built.outcome, Outcome::Handled("Echo"));
        assert_eq!(ipmb::parse(&frame).unwrap().data, &[7, 8][..]);
        assert!(!ext.asked, "the extension was asked about a Tier-1 row");
        // A command the table lacks goes to the extension.
        let mut ext = ExtAnswer::new(NETFN_APP, 0x20, Ok(2));
        let (built, frame) = ext_response(&table, &mut ext, (NETFN_APP, 0x20, 0), &[9]).unwrap();
        assert_eq!(built.outcome, Outcome::Handled("Ext Test"));
        assert_eq!(built.completion_code, cc::OK);
        assert_eq!(ipmb::parse(&frame).unwrap().data, &[ExtAnswer::FILL; 2][..]);
        assert_eq!(ext.seen, [9]);
    }

    #[test]
    fn extension_error_code_has_no_data() {
        let mut ext = ExtAnswer::new(NETFN_APP, 0x20, Err(cc::PARAM_OUT_OF_RANGE));
        let (built, frame) = ext_response(&[], &mut ext, (NETFN_APP, 0x20, 0), &[]).unwrap();
        assert_eq!(built.completion_code, cc::PARAM_OUT_OF_RANGE);
        assert!(ipmb::parse(&frame).unwrap().data.is_empty());
    }

    #[test]
    fn extension_over_long_length_fails_as_a_row_does() {
        let too_long = ipmb::IPMB_RESPONSE_DATA_MAX + 1;
        let mut ext = ExtAnswer::new(NETFN_APP, 0x20, Ok(too_long));
        let from_ext = ext_response(&[], &mut ext, (NETFN_APP, 0x20, 0), &[]).map(|r| r.0);
        let table: [Command; 1] = [cmd(NETFN_APP, 0x20, "Long", "test", overlong_handler)];
        let from_row = ext_response(&table, &mut NoExtension, (NETFN_APP, 0x20, 0), &[]);
        assert_eq!(from_ext, Err(IpmbError::TooLong));
        assert_eq!(from_row.map(|r| r.0), Err(IpmbError::TooLong));
    }

    #[test]
    fn extension_group_extension_command_sees_the_data_after_the_vso_byte() {
        let mut ext = ExtAnswer::new(NETFN_GROUP_EXT, 0x77, Ok(2));
        let data = [VSO_IDENTIFIER, 0x55, 0x66];
        let (built, frame) =
            ext_response(&[], &mut ext, (NETFN_GROUP_EXT, 0x77, 0), &data).unwrap();
        assert_eq!(ext.seen, [0x55, 0x66]);
        assert_eq!(built.completion_code, cc::OK);
        assert_eq!(
            ipmb::parse(&frame).unwrap().data,
            &[VSO_IDENTIFIER, ExtAnswer::FILL, ExtAnswer::FILL][..]
        );
    }

    #[test]
    fn extension_is_not_reached_for_another_lun_or_a_foreign_defining_body() {
        let mut ext = ExtAnswer::new(NETFN_APP, 0x20, Ok(0));
        let (built, _) = ext_response(&[], &mut ext, (NETFN_APP, 0x20, 2), &[]).unwrap();
        assert_eq!(built.completion_code, cc::INVALID_FOR_LUN);
        let mut ext = ExtAnswer::new(NETFN_GROUP_EXT, 0x77, Ok(0));
        let (built, _) = ext_response(&[], &mut ext, (NETFN_GROUP_EXT, 0x77, 0), &[0x00]).unwrap();
        assert_eq!(built.outcome, Outcome::BadVso);
        assert!(!ext.asked, "the extension was asked past a failed check");
    }

    #[test]
    fn no_extension_answers_c1() {
        let (built, _) = ext_response(&[], &mut NoExtension, (NETFN_APP, 0x20, 0), &[]).unwrap();
        assert_eq!(
            (built.completion_code, built.outcome),
            (cc::INVALID_CMD, Outcome::NoHandler)
        );
    }

    #[test]
    fn warm_reset_clears_the_hand_offs_and_keeps_the_rest() {
        let mut c = context();
        c.pending_ipmb = Some(PendingIpmbState {
            a: None,
            b: None,
            speed_a: None,
            speed_b: None,
        });
        c.pending_power = Some(PowerRequest::On);
        c.pending_payload_reset = true;
        c.pending_reset = Some(ResetRequest::Warm);
        c.deactivation_requested = true;
        c.payload_mode = PayloadMode::new(5, 2, sensor::TransitionStatus::Failed).unwrap();
        c.set_nv_settings(0b11);
        c.event_enables.get_mut(2).unwrap().enabled = !c.event_enables.get(2).unwrap().enabled;
        let (fru, sdr_count, self_test, state, enables) = (
            c.fru.bytes().to_vec(),
            c.sdr.record_count(),
            c.self_test,
            c.fru_state,
            c.event_enables,
        );
        let mut view = c.link;
        view.buses.b.mode = crate::link::BusMode::Isolated;
        c.warm_reset(view);
        assert_eq!(c.link, view);
        assert!(c.pending_ipmb.is_none() && c.pending_power.is_none());
        assert!(!c.pending_payload_reset && c.pending_reset.is_none());
        // Kept: FRU store and settings byte, SDR, self test, FRU state, event enables, payload
        // mode, the standing deactivation request.
        assert_eq!(c.fru.bytes(), fru.as_slice());
        assert_eq!(c.nv_settings(), 0b11);
        assert_eq!(c.sdr.record_count(), sdr_count);
        assert_eq!(c.self_test, self_test);
        assert_eq!(c.fru_state, state);
        assert_eq!(c.event_enables, enables);
        assert_eq!(c.payload_mode.current(), 5);
        assert!(c.deactivation_requested);
    }

    #[test]
    fn the_settings_byte_is_the_store_byte_and_a_change_marks_it_dirty() {
        let mut c = context();
        assert_eq!(c.nv_settings(), 0, "power-up: no stored record");
        assert!(!c.fru.is_dirty());
        c.set_nv_settings(0b10);
        assert_eq!(c.nv_settings(), 0b10);
        assert!(c.fru.is_dirty(), "the flash log saves it like a FRU write");
        assert_eq!(c.fru.nv_settings(), 0b10);
    }

    #[test]
    fn sensor_7_reads_only_at_tier_3_and_follows_payload_mode() {
        let mut c = context();
        c.payload_mode = PayloadMode::new(8, 3, sensor::TransitionStatus::InProgress).unwrap();
        let buses = c.link.buses;
        // (tier, result): sensor 7 is NotPresent below Tier-3; at Tier-3 four bytes: NVMRO
        // clear, in progress (00b), previous mode 3, mode 8 in the MS byte with bit 7 set.
        for tier in [1u32, 2, 3] {
            let mut out = [0xEE; sensor::READING_LEN_MAX];
            let got = c.read_sensor_at(tier, 7, &buses, &mut out);
            if tier == 3 {
                assert_eq!(got, Ok(4));
                // Scanning (status bit 6); bit 7 follows the enables of the build's Tier.
                assert_eq!(
                    [out[0], out[1] & 0x7F, out[2], out[3]],
                    [0x03, 0x40, 0x00, 0x81]
                );
            } else {
                assert_eq!(got, Err(SensorReadError::NotPresent), "tier {tier}");
            }
            let mut other = [0u8; sensor::READING_LEN_MAX];
            assert_eq!(
                c.read_sensor_at(tier, 8, &buses, &mut other),
                Err(SensorReadError::NotPresent)
            );
        }
        let mut short = [0u8; sensor::READING_LEN];
        assert_eq!(
            c.read_sensor_at(3, 7, &buses, &mut short),
            Err(SensorReadError::OutputTooSmall)
        );
        c.addr.nvmro_asserted = true;
        let mut out = [0u8; sensor::READING_LEN_MAX];
        c.read_sensor_at(3, 7, &buses, &mut out).unwrap();
        assert_eq!(out[0], 0x43, "the NVMRO state is bit 6");
    }

    #[test]
    fn a_deactivation_request_reaches_m5_only_where_activation_control_exists() {
        // Drive the machine to M4 with the sensor feed, then raise the request.
        let mut c = context();
        let mut on = SensorSnapshot::EMPTY;
        on.power = PowerState::On;
        on.payload_shut_down = false;
        for now_ms in [1u32, 2, 3] {
            c.update_sensors(now_ms, &on, &mut NoTrace);
        }
        assert_eq!(c.fru_state.state(), crate::fru_state::State::M4);
        c.deactivation_requested = true;
        c.update_sensors(4, &on, &mut NoTrace);
        let expected = if tier::activation_control(ipmc_defs::IPMC_TIER) {
            crate::fru_state::State::M5
        } else {
            crate::fru_state::State::M4
        };
        assert_eq!(c.fru_state.state(), expected);
    }
}

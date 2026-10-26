//! link.rs - dual-bus IPMB link layer (PLAN.md 3.3). No hardware: the bus drivers (Phase 6) hand
//! received frames in with `on_frame` and take response frames out with `next_tx` and
//! `on_tx_result`.
//!
//! ```text
//! driver --on_frame--> ingress FIFO --poll--> duplicate cache --miss--> dispatch --> egress queue
//!                                                  | hit (either bus)                 ^
//!                                                  +----- cached response ------------+
//! egress queue --next_tx--> driver --on_tx_result--> pop, or resend on the other bus
//! ```
//!
//! VITA 46.11 rules implemented here: 4.1.1-4 power-up A on / B off; 4.1.1-5 requests accepted
//! on both buses; 4.1.1-6 transmissions alternate between the enabled buses (the bus is chosen
//! when a response is first handed to a driver, so a resend counts as the latest transmission);
//! 4.1.1-7 a failed transmit is resent on the other bus; 4.1.5.4.1-5 an invalid hardware
//! address leaves both buses isolated; 4.1.8-3 a Set IPMB State that isolates both buses is
//! refused; 9.3-10 bus state changes only through `set_ipmb_state` or a persistent fault
//! (`set_local_status`; the fault detection itself is Phase 6 and uses the per-bus counters).
//!
//! Interrupt context (PLAN.md 3.3a): the Link never runs in an interrupt. Each bus's I2C
//! interrupt (Phase 6) only moves bytes, stamps the STOP time and posts the frame to that bus's
//! single-producer single-consumer queue; the main loop drains both queues in arrival order into
//! `on_frame`, then calls `poll` and `next_tx` / `on_tx_result`. The firmware keeps the Link in
//! a static owned by one module (`ipmb_link.rs`). `on_frame` still copies at most IPMB_MSG_MAX
//! bytes, bumps counters and never traces (drops are logged to a small FIFO that `poll` prints).
//! Nothing here blocks or waits.

use crate::addr::AddrInfo;
use crate::cmd::app::CMD_GET_DEVICE_ID;
use crate::commands::{Command, COMMANDS};
use crate::dispatch::{dispatch_with, Context};
use crate::dupcache::{CacheEntry, DupCache, Key};
use crate::ext::Extension;
use crate::ipmb::{self, IpmbError, IPMB_MAX_LEN, NETFN_APP};
use crate::names::{CommandName, CoreNames, NetFnName, TxFault};
use crate::queue::{Bus, EgressFrame, EgressQueue, Fifo};
use crate::requester::Requester;
use crate::sensor::{LinkBus, LinkSensor, LocalStatus};
use crate::trace::Trace;

/// BROADCAST_GET_DEVICE_ID -- true once a general-call Get Device ID is answered end to end
/// (`on_frame` strips the 00h byte and keeps the probe whose rsSA is this controller's address).
/// tools/sync_ipmi_commands.py reads this line to mark the Broadcast row implemented.
pub const BROADCAST_GET_DEVICE_ID: bool = true;

/// INGRESS_DEPTH -- received requests held between the driver and the main loop (defs.rs).
pub const INGRESS_DEPTH: usize = ipmc_defs::IPMB_INGRESS_DEPTH;
/// TX_RETRIES -- most resends of one response frame (defs.rs: a u8, guarded 3..=10).
pub const TX_RETRIES: u8 = ipmc_defs::IPMB_TX_RETRIES;
/// RESPONSE_DEADLINE_MS -- T5max, request STOP to response STOP (VITA 46.11 Table 4.1.1-1,
/// Rule 4.1.1-11). A response whose transmission completes later than this after the request
/// STOP is late (`Link::late_responses`).
pub const RESPONSE_DEADLINE_MS: u32 = 227;

/// CM_ADDR8 -- the Chassis Manager's 8-bit IPMB address (defs.rs IPMB_CM_ADDR8, guarded
/// 2..=0xFE with bit 0 clear in ipmc-defs, so the cast is exact).
pub const CM_ADDR8: u8 = ipmc_defs::IPMB_CM_ADDR8;

/// General-call address byte that precedes a broadcast frame (IPMI 1.5 section 17.9).
const GENERAL_CALL: u8 = 0x00;
/// TRACE_ON -- IPMC_IPMI_TRACE from defs.rs; false removes every trace call.
const TRACE_ON: bool = ipmc_defs::IPMC_IPMI_TRACE;

/// TxOutcome -- what a driver reports for one transmission: sent, or failed with the cause the
/// master saw. A plain `bool` converts (true sent, false an unspecified failure).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxOutcome {
    /// The frame was transmitted.
    Sent,
    /// The transmission failed.
    Failed(TxFault),
}

impl From<bool> for TxOutcome {
    fn from(sent: bool) -> Self {
        if sent {
            TxOutcome::Sent
        } else {
            TxOutcome::Failed(TxFault::Unspecified)
        }
    }
}

/// LocalError -- why `local_request` gave no response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalError {
    /// The request does not fit one IPMB frame.
    TooLong,
    /// The dispatcher produced no response.
    Dispatch,
}

impl LocalError {
    /// name -- log name of the error.
    pub const fn name(self) -> &'static str {
        match self {
            LocalError::TooLong => "TooLong",
            LocalError::Dispatch => "Dispatch",
        }
    }
}

/// RequestWho -- `to 20h (Chassis Manager)`: the destination of a request this IPMC sent.
struct RequestWho(u8);

impl core::fmt::Display for RequestWho {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "to {:02X}h", self.0)?;
        if self.0 == CM_ADDR8 {
            f.write_str(" (Chassis Manager)")?;
        }
        Ok(())
    }
}

/// RequestWhat -- `S/E Platform Event seq 0Bh`: the command of a request this IPMC sent, named
/// from the tables the link reaches without the extension.
struct RequestWhat {
    netfn: u8,
    cmd: u8,
    seq: u8,
}

impl core::fmt::Display for RequestWhat {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{} {} seq {:02X}h",
            NetFnName(self.netfn),
            CommandName {
                names: &CoreNames(&crate::ext::NoExtension),
                netfn: self.netfn,
                cmd: self.cmd
            },
            self.seq
        )
    }
}

/// TxPolicy -- how the bus of a response is chosen (IPMB_TX_POLICY in ipmc-defs): Alternate
/// sends successive transmissions on the enabled buses in turn (VITA 46.11 Rule 4.1.1-6),
/// Arrival sends the response on the bus the request arrived on.
pub use ipmc_defs::TxPolicy;

/// tx_policy_name -- text of a policy for console lines.
pub const fn tx_policy_name(policy: TxPolicy) -> &'static str {
    match policy {
        TxPolicy::Alternate => "alternate",
        TxPolicy::Arrival => "arrival",
    }
}

/// LinkConfig -- build choices of the link layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkConfig {
    /// IPMB-B is built in (IPMB_BUS_B_ENABLE). IPMB-A is always present.
    pub b_fitted: bool,
    /// Response bus policy.
    pub tx_policy: TxPolicy,
    /// The requester may send requests: Tier 2 and 3 (VITA 46.11 Rule 7-3); a Tier-1 IPMC sends
    /// none (Rule 7-4).
    pub requests_allowed: bool,
}

impl LinkConfig {
    /// from_defs -- the configuration of ipmc-defs.
    pub const fn from_defs() -> LinkConfig {
        LinkConfig {
            b_fitted: ipmc_defs::IPMB_BUS_B_ENABLE,
            tx_policy: ipmc_defs::IPMB_TX_POLICY,
            requests_allowed: ipmc_defs::IPMC_TIER >= 2,
        }
    }
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self::from_defs()
    }
}

/// BusMode -- the Set IPMB State value of one bus (Set IPMB State request bytes 2 and 3, bit 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusMode {
    /// Override: the bus is isolated (disabled).
    Isolated,
    /// Local Control: the IPMC decides; a bus without a local fault is enabled.
    LocalControl,
}

impl BusMode {
    /// name -- text for console lines.
    pub const fn name(self) -> &'static str {
        match self {
            BusMode::Isolated => "isolated",
            BusMode::LocalControl => "local control",
        }
    }
}

/// BusSpeed -- the IPMB speed of one bus (Set IPMB State speed byte, 2 bits per bus).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusSpeed {
    /// 00b: STANDARD-MODE, 100 kHz (Rule 4.1.1-4.3: the power-up speed of every bus).
    Standard,
    /// 01b: FAST-MODE, 400 kHz (Tier-3, Rule 4.1.1-4.4).
    Fast,
}

impl BusSpeed {
    /// name -- text for console lines.
    pub const fn name(self) -> &'static str {
        match self {
            BusSpeed::Standard => "100 kHz",
            BusSpeed::Fast => "400 kHz",
        }
    }
}

/// BusState -- state of one bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusState {
    /// The bus is built in.
    pub fitted: bool,
    /// Set IPMB State value.
    pub mode: BusMode,
    /// The speed the bus is set to (Set IPMB State speed byte); the owner of the master driver
    /// applies it between transfers.
    pub speed: BusSpeed,
    /// Persistent local fault, `LocalStatus::None` when healthy.
    pub status: LocalStatus,
}

impl BusState {
    /// healthy -- a bus without a local fault.
    const fn healthy(fitted: bool, mode: BusMode) -> BusState {
        BusState {
            fitted,
            mode,
            speed: BusSpeed::Standard,
            status: LocalStatus::None,
        }
    }

    /// enabled -- the bus carries traffic: fitted, in Local Control and without a local fault.
    pub fn enabled(&self) -> bool {
        self.fitted && self.mode == BusMode::LocalControl && self.status == LocalStatus::None
    }
}

/// PerBus -- one value for each bus, reached by `Bus` so no index can be out of range. The one
/// per-bus container: bus states, counters and flags all use it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PerBus<T> {
    /// The value for IPMB-A.
    pub a: T,
    /// The value for IPMB-B.
    pub b: T,
}

impl<T> PerBus<T> {
    /// get -- the value of `bus`.
    pub fn get(&self, bus: Bus) -> &T {
        match bus {
            Bus::A => &self.a,
            Bus::B => &self.b,
        }
    }

    /// get_mut -- the value of `bus` for change.
    pub fn get_mut(&mut self, bus: Bus) -> &mut T {
        match bus {
            Bus::A => &mut self.a,
            Bus::B => &mut self.b,
        }
    }
}

/// BusPair -- the state of both buses.
pub type BusPair = PerBus<BusState>;

impl PerBus<BusState> {
    /// power_up -- IPMB-A Local Control and enabled, IPMB-B isolated (VITA 46.11 Rule 4.1.1-4),
    /// fitted only when `b_fitted`. An invalid hardware address (`address_usable` false) leaves
    /// IPMB-A isolated as well, so neither bus accepts or sends a frame (Rule 4.1.5.4.1-5).
    pub const fn power_up(b_fitted: bool, address_usable: bool) -> BusPair {
        let a_mode = if address_usable {
            BusMode::LocalControl
        } else {
            BusMode::Isolated
        };
        BusPair {
            a: BusState::healthy(true, a_mode),
            b: BusState::healthy(b_fitted, BusMode::Isolated),
        }
    }

    /// sensor_data -- both buses as the IPMB Link sensor reports them (`sensor::encode_link`).
    pub fn sensor_data(&self) -> LinkSensor {
        let bus = |state: &BusState| LinkBus {
            local_control: state.mode == BusMode::LocalControl,
            status: state.status,
            enabled: state.enabled(),
            fast: state.speed == BusSpeed::Fast,
        };
        LinkSensor {
            a: bus(&self.a),
            b: bus(&self.b),
        }
    }
}

/// plan_ipmb_state -- the ONE rule of Set IPMB State: the bus states that would result from
/// applying `a` and `b` (`Some(mode)` sets that bus, `None` leaves it, request byte FFh) to
/// `current`. All or nothing: Err(BusNotFitted) when Local Control is asked for a bus that is not
/// built in (Rule 4.1.1-14: unsupported); Err(IsolatesBothBuses) when no bus would stay enabled
/// (Rule 4.1.8-3). `Link::set_ipmb_state` applies the result; the Set IPMB State handler
/// (cmd/vita.rs) calls this on the Link's view to choose its completion code.
pub fn plan_ipmb_state(
    current: &BusPair,
    a: Option<BusMode>,
    b: Option<BusMode>,
) -> Result<BusPair, LinkError> {
    let next = plan_per_bus(current, a, b, |state, mode| {
        if mode == BusMode::LocalControl && !state.fitted {
            return Err(LinkError::BusNotFitted);
        }
        state.mode = mode;
        Ok(())
    })?;
    if !next.a.enabled() && !next.b.enabled() {
        return Err(LinkError::IsolatesBothBuses);
    }
    Ok(next)
}

/// plan_ipmb_speed -- the bus states that would result from setting the speeds `a` and `b`
/// (`Some(speed)` sets that bus, `None` leaves it) in `current`. All or nothing:
/// Err(BusNotFitted) when FAST-MODE is asked for a bus that is not built in (Rule 4.1.1-14:
/// unsupported). Standard on any bus is always accepted. The Set IPMB State handler calls this
/// on the Link's view to choose its completion code; `Link::set_ipmb_speed` applies the result.
pub fn plan_ipmb_speed(
    current: &BusPair,
    a: Option<BusSpeed>,
    b: Option<BusSpeed>,
) -> Result<BusPair, LinkError> {
    plan_per_bus(current, a, b, |state, speed| {
        if speed == BusSpeed::Fast && !state.fitted {
            return Err(LinkError::BusNotFitted);
        }
        state.speed = speed;
        Ok(())
    })
}

/// plan_per_bus -- applies the requests `a` and `b` (`Some(value)` sets that bus, `None` leaves
/// it) to a copy of `current` with `change`, bus A first: the copy, or the first error. The one
/// loop of the Set IPMB State rules (`plan_ipmb_state`, `plan_ipmb_speed`).
fn plan_per_bus<T>(
    current: &BusPair,
    a: Option<T>,
    b: Option<T>,
    change: impl Fn(&mut BusState, T) -> Result<(), LinkError>,
) -> Result<BusPair, LinkError> {
    let mut next = *current;
    if let Some(value) = a {
        change(&mut next.a, value)?;
    }
    if let Some(value) = b {
        change(&mut next.b, value)?;
    }
    Ok(next)
}

/// PendingIpmbState -- a Set IPMB State change the handler validated and left in
/// `Context::pending_ipmb`. The hand-off: the handler runs inside `Link::poll`, which gives it a
/// read-only `LinkView` before the call; the handler cannot reach the Link, so it records the
/// change here; `Link::poll` applies it (`Link::set_ipmb_state`) right after the handler
/// returns, before the response is queued, so the response leaves on the bus state the command
/// just set (Rule 9.3-10: the state changes only on Set IPMB State).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingIpmbState {
    /// New mode of IPMB-A, None = no change.
    pub a: Option<BusMode>,
    /// New mode of IPMB-B, None = no change.
    pub b: Option<BusMode>,
    /// New speed of IPMB-A, None = no change (speed byte absent, or Tier-1/2).
    pub speed_a: Option<BusSpeed>,
    /// New speed of IPMB-B, None = no change.
    pub speed_b: Option<BusSpeed>,
}

/// LinkView -- what a handler may read of the Link during one dispatch: the build configuration
/// and both bus states. `Link::poll` refreshes it before every dispatch; handlers never write it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkView {
    /// Build configuration (IPMB-B fitted or not).
    pub config: LinkConfig,
    /// State of both buses.
    pub buses: BusPair,
}

impl LinkView {
    /// power_up -- the view of a fresh link with `config`; `address_usable` is the hardware
    /// address decision (`AddrInfo::usable`), see `BusPair::power_up`.
    pub const fn power_up(config: LinkConfig, address_usable: bool) -> LinkView {
        LinkView {
            config,
            buses: BusPair::power_up(config.b_fitted, address_usable),
        }
    }
}

/// LinkError -- why a link call was refused; state is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// The request would leave neither bus enabled (VITA 46.11 Rule 4.1.8-3).
    IsolatesBothBuses,
    /// Local Control was requested for a bus that is not built in (Rule 4.1.1-14: unsupported).
    BusNotFitted,
    /// A transmit result arrived while no frame was waiting.
    NoTxInFlight,
    /// The result names a different bus than the one the waiting frame was handed out on.
    TxBusMismatch,
}

impl LinkError {
    /// name -- log name of the error.
    pub const fn name(self) -> &'static str {
        match self {
            LinkError::IsolatesBothBuses => "IsolatesBothBuses",
            LinkError::BusNotFitted => "BusNotFitted",
            LinkError::NoTxInFlight => "NoTxInFlight",
            LinkError::TxBusMismatch => "TxBusMismatch",
        }
    }
}

/// FrameDrop -- why `on_frame` did not queue a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameDrop {
    /// The bus is disabled or not built in.
    BusUnavailable,
    /// The frame failed `ipmb::parse`.
    Parse(IpmbError),
    /// The frame is a response that answers no request of ours (a Tier-1 IPMC sends none).
    Response,
    /// The request is addressed to another controller (or is a broadcast other than Get Device ID).
    NotForUs,
    /// The ingress FIFO was full.
    IngressFull,
}

impl FrameDrop {
    /// name -- log name of the reason.
    pub const fn name(self) -> &'static str {
        match self {
            FrameDrop::BusUnavailable => "BusUnavailable",
            FrameDrop::Parse(e) => e.name(),
            FrameDrop::Response => "Response",
            FrameDrop::NotForUs => "NotForUs",
            FrameDrop::IngressFull => "IngressFull",
        }
    }
}

/// RxErrors -- frames rejected by `ipmb::parse`, by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RxErrors {
    /// Shorter than the minimum request.
    pub too_short: u32,
    /// Longer than IPMB_MSG_MAX.
    pub too_long: u32,
    /// chk1 wrong.
    pub header_checksum: u32,
    /// chk2 wrong.
    pub data_checksum: u32,
    /// An address byte with bit 0 set.
    pub invalid_address: u32,
    /// Any other parse error.
    pub other: u32,
}

impl RxErrors {
    /// count -- adds one to the field that counts `error` (saturating).
    fn count(&mut self, error: IpmbError) {
        let bump = |field: &mut u32| *field = field.saturating_add(1);
        match error {
            IpmbError::TooShort => bump(&mut self.too_short),
            IpmbError::TooLong => bump(&mut self.too_long),
            IpmbError::HeaderChecksum => bump(&mut self.header_checksum),
            IpmbError::DataChecksum => bump(&mut self.data_checksum),
            IpmbError::InvalidAddress => bump(&mut self.invalid_address),
            _ => bump(&mut self.other),
        }
    }

    /// total -- all parse errors.
    pub fn total(&self) -> u32 {
        [
            self.too_short,
            self.too_long,
            self.header_checksum,
            self.data_checksum,
            self.invalid_address,
            self.other,
        ]
        .iter()
        .fold(0, |sum, n| sum.saturating_add(*n))
    }
}

/// BusCounters -- everything one bus has done since start; feeds the IPMB Link sensor and the
/// persistent-fault detection of Phase 6 (which sets its own thresholds).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BusCounters {
    /// Requests accepted into the ingress FIFO.
    pub rx_ok: u32,
    /// Frames rejected by the parser, by kind.
    pub rx_errors: RxErrors,
    /// Valid requests addressed to another controller.
    pub not_for_us: u32,
    /// Valid responses that answer no request of ours (dropped).
    pub rx_response: u32,
    /// Valid responses that answered the outstanding request (`Requester`).
    pub rx_answers: u32,
    /// Frames that arrived while the bus was disabled or not built in.
    pub bus_unavailable: u32,
    /// Valid requests lost because the ingress FIFO was full.
    pub ingress_full: u32,
    /// Frames transmitted successfully.
    pub tx_ok: u32,
    /// Request transmissions on this bus, failed ones included (the first and every retry).
    pub tx_requests: u32,
    /// Transmit attempts that failed.
    pub tx_fail: u32,
    /// Failed transmits since the last success on this bus.
    pub tx_fail_streak: u32,
    /// Frames given up after the last allowed resend (or with no bus to resend on).
    pub tx_dropped: u32,
    /// Resends that went out on this bus after a failure on the other.
    pub resends: u32,
    /// Retried requests answered from the duplicate cache; counted on the arrival bus.
    pub replays: u32,
}

/// One received request waiting for `poll`.
#[derive(Clone, Copy)]
struct Pending {
    bytes: [u8; IPMB_MAX_LEN],
    len: usize,
    bus: Bus,
    arrival_ms: u32,
}

impl Pending {
    const EMPTY: Pending = Pending {
        bytes: [0; IPMB_MAX_LEN],
        len: 0,
        bus: Bus::A,
        arrival_ms: 0,
    };
}

/// One dropped frame waiting to be traced by `poll`.
#[derive(Clone, Copy)]
struct DropRecord {
    bus: Bus,
    reason: FrameDrop,
    len: usize,
}

impl DropRecord {
    const EMPTY: DropRecord = DropRecord {
        bus: Bus::A,
        reason: FrameDrop::Response,
        len: 0,
    };
}

/// Link -- the dual-bus link layer.
pub struct Link {
    config: LinkConfig,
    /// Own 8-bit IPMB address; `on_frame` has no Context, so it is copied at start-up.
    own_addr: u8,
    buses: BusPair,
    counters: PerBus<BusCounters>,
    ingress: Fifo<Pending, INGRESS_DEPTH>,
    drops: Fifo<DropRecord, INGRESS_DEPTH>,
    cache: DupCache,
    egress: EgressQueue,
    /// Bus of the latest frame handed to a driver (a resend counts); ALTERNATE picks the other
    /// one for the next new frame.
    last_tx_bus: Bus,
    /// Responses whose transmission completed after RESPONSE_DEADLINE_MS.
    late_responses: u32,
    dispatch_errors: u32,
    /// Requests this IPMC sends (Tier-2/3); idle in a Tier-1 build. It also holds the bus of the
    /// request frame handed to a driver whose outcome is open (`Requester::sending_bus`).
    requester: Requester,
}

/// REQUESTS_BUILT -- this build can send requests: Tier 2 or 3 (defs.rs IPMC_TIER), and every
/// unit test of the requester path. A Tier-1 image never reaches the requester code behind this
/// constant, so the optimiser drops it.
const REQUESTS_BUILT: bool = cfg!(test) || ipmc_defs::IPMC_TIER >= 2;

impl Link {
    /// new -- a link in the power-up state: IPMB-A Local Control and enabled, IPMB-B isolated
    /// (VITA 46.11 Rule 4.1.1-4) and fitted only when `config.b_fitted`. `addr` supplies the
    /// own IPMB address; an invalid hardware address (bad parity or GA 0) isolates both buses
    /// (Rule 4.1.5.4.1-5), so `on_frame` drops everything and nothing is sent.
    pub const fn new(config: LinkConfig, addr: &AddrInfo) -> Link {
        Self::with_first_request_seq(config, addr, 0, 0, 0)
    }

    /// with_first_request_seq -- `new` with the rqSeq of the first request this IPMC sends and
    /// the rqSeq values the previous run sent inside T9 (`recent`, bit n for rqSeq n, from
    /// `recent_request_seqs`), booked as sent at `start_ms` so none is used again for a full T9
    /// (`Requester::with_recent_seqs`): the owner of the processor hands over what it kept across
    /// a Cold Reset.
    pub const fn with_first_request_seq(
        config: LinkConfig,
        addr: &AddrInfo,
        first_seq: u8,
        recent: u64,
        start_ms: u32,
    ) -> Link {
        Link {
            config,
            own_addr: addr.ipmb_address,
            buses: BusPair::power_up(config.b_fitted, addr.usable()),
            counters: PerBus {
                a: BusCounters::ZERO,
                b: BusCounters::ZERO,
            },
            ingress: Fifo::new(Pending::EMPTY),
            drops: Fifo::new(DropRecord::EMPTY),
            cache: DupCache::new(),
            egress: EgressQueue::new(),
            // ALTERNATE starts on A: the first choice is the other bus than this one.
            last_tx_bus: Bus::B,
            late_responses: 0,
            dispatch_errors: 0,
            requester: Requester::new(config.requests_allowed, first_seq)
                .with_own_address(addr.ipmb_address)
                .with_recent_seqs(recent, start_ms),
        }
    }

    /// restarted -- the Link of a restart of the IPMC's link state at `now_ms`: the same
    /// configuration and `addr`, the power-up bus state (Rule 4.1.1-4), empty queues and caches,
    /// and the request sequence state of this link carried over (`next_request_seq`,
    /// `recent_request_seqs`), so no rqSeq is reused inside T9. The one rebuild both the Warm
    /// Reset owners and the emulator's Cold Reset use.
    pub fn restarted(&self, addr: &AddrInfo, now_ms: u32) -> Link {
        Link::with_first_request_seq(
            self.config,
            addr,
            self.next_request_seq(),
            self.recent_request_seqs(now_ms),
            now_ms,
        )
    }

    /// next_request_seq -- the rqSeq the next request gets, for the owner of the processor to
    /// keep across a Cold Reset.
    pub const fn next_request_seq(&self) -> u8 {
        self.requester.next_seq()
    }

    /// recent_request_seqs -- the rqSeq values sent less than T9 before `now_ms` (bit n for rqSeq
    /// n), for the owner of the processor to keep across a Cold Reset with `next_request_seq`.
    pub fn recent_request_seqs(&self, now_ms: u32) -> u64 {
        self.requester.recent_seqs(now_ms)
    }

    // ---- ingress ----

    /// on_frame -- the main loop hands in each frame a driver received on `bus`, STOP at `now_ms`.
    /// 1. A bus that is disabled or not built in drops the frame.
    /// 2. A leading general-call byte 00h is stripped (Broadcast Get Device ID, IPMI v2.0
    ///    section 20.9).
    /// 3. `ipmb::parse` validates it; a response goes to the requester, which keeps it when it
    ///    answers the outstanding request (`BusCounters::rx_answers`, Ok) and otherwise drops it;
    ///    a request must carry this controller's address in rsSA, and a broadcast must also be
    ///    Get Device ID.
    /// 4. The accepted request joins the ingress FIFO with its bus and `now_ms`; a full FIFO
    ///    drops the new frame and keeps the queued ones.
    ///
    /// Every drop is counted per bus and logged for `poll` to trace. Err(reason) is the drop;
    /// Ok(()) is queued. Called from the main loop (module comment).
    pub fn on_frame(&mut self, bus: Bus, frame: &[u8], now_ms: u32) -> Result<(), FrameDrop> {
        let result = self.accept(bus, frame, now_ms);
        if let (Err(reason), true) = (result, TRACE_ON) {
            // A full log loses only this trace line; the counters keep the truth.
            let _ = self.drops.push(DropRecord {
                bus,
                reason,
                len: frame.len(),
            });
        }
        result
    }

    fn accept(&mut self, bus: Bus, frame: &[u8], now_ms: u32) -> Result<(), FrameDrop> {
        if !self.buses.get(bus).enabled() {
            let counters = self.counters.get_mut(bus);
            counters.bus_unavailable = counters.bus_unavailable.saturating_add(1);
            return Err(FrameDrop::BusUnavailable);
        }
        let (body, broadcast) = match frame.split_first() {
            Some((&GENERAL_CALL, rest)) => (rest, true),
            _ => (frame, false),
        };
        let request = match ipmb::parse(body) {
            Ok(request) => request,
            Err(error) => {
                self.counters.get_mut(bus).rx_errors.count(error);
                return Err(FrameDrop::Parse(error));
            }
        };
        let counters = self.counters.get_mut(bus);
        if request.header.is_response() {
            if REQUESTS_BUILT && self.requester.on_response(&request) {
                counters.rx_answers = counters.rx_answers.saturating_add(1);
                return Ok(());
            }
            counters.rx_response = counters.rx_response.saturating_add(1);
            return Err(FrameDrop::Response);
        }
        let is_get_device_id =
            request.header.netfn == NETFN_APP && request.header.cmd == CMD_GET_DEVICE_ID;
        // A broadcast is a discovery probe: the requester cycles rsSA through the addresses, and
        // only the controller the rsSA names answers (IPMI v2.0 section 20.9).
        let addressed_to_us =
            request.header.dest == self.own_addr && (!broadcast || is_get_device_id);
        if !addressed_to_us {
            counters.not_for_us = counters.not_for_us.saturating_add(1);
            return Err(FrameDrop::NotForUs);
        }
        let mut pending = Pending::EMPTY;
        if let Some(dst) = pending.bytes.get_mut(..body.len()) {
            dst.copy_from_slice(body);
        }
        pending.len = body.len();
        pending.bus = bus;
        pending.arrival_ms = now_ms;
        if self.ingress.push(pending).is_err() {
            counters.ingress_full = counters.ingress_full.saturating_add(1);
            return Err(FrameDrop::IngressFull);
        }
        counters.rx_ok = counters.rx_ok.saturating_add(1);
        Ok(())
    }

    // ---- main loop: requests to responses ----

    /// poll -- `poll_with` over the COMMANDS table; `ext` answers what the table does not
    /// (`NoExtension` in the Tier-1 build).
    pub fn poll(
        &mut self,
        ctx: &mut Context,
        ext: &mut dyn Extension,
        now_ms: u32,
        trace: &mut dyn Trace,
    ) -> usize {
        self.poll_with(COMMANDS, ext, ctx, now_ms, trace)
    }

    /// poll_with -- main-loop step using `table`.
    /// 1. Traces the frames `on_frame` dropped since the last call.
    /// 2. Takes ingress requests oldest first (both buses merged, arrival order). For each:
    ///    a repeat inside IPMB_DUP_WINDOW_MS re-queues the cached response and runs no
    ///    handler; anything else goes through `dispatch_with` and its response is cached.
    ///    The response bus is chosen later, by `next_tx`, when the frame first goes to a driver.
    /// 3. Stops when the egress queue is full; the rest wait in the ingress FIFO.
    /// 4. Steps the requester timing (a retransmission falls due, or the request ends with
    ///    NoResponse) and traces a request that ended.
    /// 5. Refreshes `ctx.link` and calls `ext.poll` once, with the requester.
    ///
    /// `now_ms` is the time of the call. Only the requester and the extension hook read it: the duplicate window
    /// runs from each request's own arrival time and the response deadline is checked when the
    /// transmission completes (`on_tx_result`). A request the table has no row for goes to
    /// `ext` (see `Extension::command`). Returns the number of requests taken.
    pub fn poll_with(
        &mut self,
        table: &[Command],
        ext: &mut dyn Extension,
        ctx: &mut Context,
        now_ms: u32,
        trace: &mut dyn Trace,
    ) -> usize {
        self.trace_drops(trace);
        let mut taken = 0;
        while !self.egress.is_full() {
            let Some(pending) = self.ingress.pop() else {
                break;
            };
            self.answer(table, ext, ctx, &pending, trace);
            taken += 1;
        }
        if REQUESTS_BUILT {
            self.requester.step(now_ms);
            self.trace_request_end(trace);
        }
        ctx.link = self.view();
        ext.poll(ctx, &mut self.requester, now_ms, trace);
        taken
    }

    /// trace_request_end -- one line when the requester's request ended since the last call.
    fn trace_request_end(&mut self, trace: &mut dyn Trace) {
        let Some(end) = self.requester.ended() else {
            return;
        };
        if !TRACE_ON {
            return;
        }
        let who = RequestWho(end.dest_addr8);
        let what = RequestWhat {
            netfn: end.netfn,
            cmd: end.cmd,
            seq: end.seq,
        };
        match end.completion_code {
            Some(code) => trace.line(format_args!(
                "request {who}: {what} answered CC {code:02X}h"
            )),
            None => trace.line(format_args!(
                "request {who}: {what} got no response after {} transmissions",
                end.transmissions
            )),
        }
    }

    fn trace_drops(&mut self, trace: &mut dyn Trace) {
        while let Some(drop) = self.drops.pop() {
            if TRACE_ON {
                trace.line(format_args!(
                    "IPMB-{} drop: {} ({} bytes)",
                    drop.bus.name(),
                    drop.reason.name(),
                    drop.len
                ));
            }
        }
    }

    fn answer(
        &mut self,
        table: &[Command],
        ext: &mut dyn Extension,
        ctx: &mut Context,
        pending: &Pending,
        trace: &mut dyn Trace,
    ) {
        let bytes = pending.bytes.get(..pending.len).unwrap_or(&[]);
        let Ok(request) = ipmb::parse(bytes) else {
            return; // on_frame parsed these bytes already
        };
        let key = Key::of(&request);
        if let Some(cached) = key
            .as_ref()
            .and_then(|key| self.cache.lookup(key, pending.arrival_ms))
        {
            let mut frame = [0u8; IPMB_MAX_LEN];
            let Some(copy) = frame.get_mut(..cached.len()) else {
                return;
            };
            copy.copy_from_slice(cached);
            let len = cached.len();
            let counters = self.counters.get_mut(pending.bus);
            counters.replays = counters.replays.saturating_add(1);
            if TRACE_ON {
                trace.line(format_args!(
                    "IPMB-{} replay seq {:02X}h FN({:02X}h) CMD({:02X}h) cached",
                    pending.bus.name(),
                    request.header.seq,
                    request.header.netfn,
                    request.header.cmd
                ));
            }
            // The replay answers the retry: the deadline runs from the retry's arrival.
            self.enqueue(
                frame.get(..len).unwrap_or(&[]),
                pending.bus,
                pending.arrival_ms,
            );
            return;
        }
        let dispatched = self.run_dispatch(
            table,
            ext,
            ctx,
            &request,
            pending.bus,
            pending.arrival_ms,
            trace,
        );
        match dispatched {
            Some(response) => {
                if let Some(key) = key {
                    self.cache
                        .insert(key, response.as_bytes(), pending.arrival_ms);
                }
                self.enqueue(response.as_bytes(), pending.bus, pending.arrival_ms);
            }
            None => self.dispatch_errors = self.dispatch_errors.saturating_add(1),
        }
    }

    /// run_dispatch -- the one dispatch of a request: refreshes `ctx.link`, runs the handler
    /// against a private queue (so the caller can cache the response before it is queued for the
    /// bus; the real queue had a free slot when `poll_with` began), then applies a Set IPMB State
    /// the handler left in `ctx.pending_ipmb`. Some(response), or None when the dispatcher
    /// produced none. `answer` (bus requests) and `local_request` (console requests) both call it.
    #[allow(clippy::too_many_arguments)] // the dispatch inputs, as dispatch_with takes them
    fn run_dispatch(
        &mut self,
        table: &[Command],
        ext: &mut dyn Extension,
        ctx: &mut Context,
        request: &ipmb::Frame,
        bus: Bus,
        arrival_ms: u32,
        trace: &mut dyn Trace,
    ) -> Option<EgressFrame> {
        let mut staged = EgressQueue::new();
        ctx.link = self.view();
        let dispatched = dispatch_with(
            table,
            ext,
            ctx,
            request,
            bus,
            arrival_ms,
            &mut staged,
            trace,
        );
        // A Set IPMB State the handler validated takes effect now, before the response is queued
        // (see `PendingIpmbState`). The plan was made on the view this dispatch started with and
        // nothing else changes the buses meanwhile, so the apply cannot be refused; a refusal
        // would leave the state as it was.
        if let Some(change) = ctx.pending_ipmb.take() {
            let _ = self.set_ipmb_state(change.a, change.b);
            let _ = self.set_ipmb_speed(change.speed_a, change.speed_b);
        }
        match (dispatched, staged.pop()) {
            (Ok(()), Some(response)) => Some(response),
            _ => None,
        }
    }

    /// local_request -- `local_request_with` over the COMMANDS table.
    #[allow(clippy::too_many_arguments)] // the request fields and the dispatch sinks
    pub fn local_request(
        &mut self,
        ctx: &mut Context,
        ext: &mut dyn Extension,
        netfn: u8,
        cmd: u8,
        data: &[u8],
        now_ms: u32,
        trace: &mut dyn Trace,
    ) -> Result<(usize, [u8; IPMB_MAX_LEN]), LocalError> {
        self.local_request_with(COMMANDS, ctx, ext, netfn, cmd, data, now_ms, trace)
    }

    /// local_request_with -- runs one request from the console as if the Chassis Manager had sent
    /// it: a frame from `CM_ADDR8` (LUN 0, sequence 0) to this controller goes through
    /// `run_dispatch` with arrival bus A, so a Set IPMB State changes the bus state as a bus
    /// request does. No duplicate-cache lookup or insert and nothing on the egress queue; the
    /// response frame comes back as (length, bytes). Err(TooLong): the request does not fit a
    /// frame; Err(Dispatch): no response.
    #[allow(clippy::too_many_arguments)] // the request fields and the dispatch sinks
    pub fn local_request_with(
        &mut self,
        table: &[Command],
        ctx: &mut Context,
        ext: &mut dyn Extension,
        netfn: u8,
        cmd: u8,
        data: &[u8],
        now_ms: u32,
        trace: &mut dyn Trace,
    ) -> Result<(usize, [u8; IPMB_MAX_LEN]), LocalError> {
        let header = ipmb::Header {
            dest: self.own_addr,
            netfn,
            dest_lun: 0,
            src: CM_ADDR8,
            seq: 0,
            src_lun: 0,
            cmd,
        };
        let mut bytes = [0u8; IPMB_MAX_LEN];
        let len =
            ipmb::encode_request(&header, data, &mut bytes).map_err(|_| LocalError::TooLong)?;
        let request =
            ipmb::parse(bytes.get(..len).unwrap_or(&[])).map_err(|_| LocalError::TooLong)?;
        let response = self
            .run_dispatch(table, ext, ctx, &request, Bus::A, now_ms, trace)
            .ok_or(LocalError::Dispatch)?;
        let mut out = [0u8; IPMB_MAX_LEN];
        let frame = response.as_bytes();
        out.get_mut(..frame.len())
            .ok_or(LocalError::Dispatch)?
            .copy_from_slice(frame);
        Ok((frame.len(), out))
    }

    /// clear_duplicate_cache -- empties the duplicate cache, so a repeat of any request runs its
    /// handler again (bench reruns of identical requests).
    pub fn clear_duplicate_cache(&mut self) {
        self.cache.clear();
    }

    /// duplicate_cache_entries -- the duplicate cache's held responses at `now_ms`, for the
    /// console.
    pub fn duplicate_cache_entries(&self, now_ms: u32) -> impl Iterator<Item = CacheEntry> + '_ {
        self.cache.entries(now_ms)
    }

    /// duplicate_cache_len -- responses held in the duplicate cache.
    pub fn duplicate_cache_len(&self) -> usize {
        self.cache.len()
    }

    /// enqueue -- puts a response frame on the egress queue with the request's arrival bus and
    /// time. The transmit bus is chosen later, in `next_tx`, so ALTERNATE follows the frames
    /// actually handed to a driver (resends included), not the order they were queued in.
    fn enqueue(&mut self, frame: &[u8], arrival: Bus, arrival_ms: u32) {
        if self.egress.push(frame, arrival, arrival_ms).is_err() {
            self.dispatch_errors = self.dispatch_errors.saturating_add(1);
        }
    }

    /// response_preference -- the bus the TX policy prefers for a response: ALTERNATE the bus
    /// other than the latest frame's (a resend counts), ARRIVAL the request's bus.
    fn response_preference(&self, arrival: Bus) -> Bus {
        match self.config.tx_policy {
            TxPolicy::Alternate => self.last_tx_bus.other(),
            TxPolicy::Arrival => arrival,
        }
    }

    /// choose_tx_bus -- the bus for a frame being handed to a driver for the first time, and the
    /// new `last_tx_bus`: the `preferred` bus, or when it is not enabled the other enabled bus
    /// (with only IPMB-A enabled, always A).
    fn choose_tx_bus(&mut self, preferred: Bus) -> Bus {
        let bus = if self.buses.get(preferred).enabled()
            || !self.buses.get(preferred.other()).enabled()
        {
            preferred
        } else {
            preferred.other()
        };
        self.last_tx_bus = bus;
        bus
    }

    // ---- main loop: egress ----

    /// next_tx -- the oldest queued response and the bus to send it on, or None when nothing
    /// waits. The frame stays queued until `on_tx_result` reports the outcome. Responses go
    /// first; with none queued, a request of the requester that is due goes out (`next_request_tx`),
    /// and the same request frame is returned until its outcome is reported.
    /// 1. A frame handed out for the first time gets its bus from the TX policy now
    ///    (`choose_tx_bus`), which makes ALTERNATE follow the real transmission order.
    /// 2. A frame already handed out whose bus has since been disabled moves to the other bus
    ///    (and that is then the latest transmission).
    ///
    /// None also when neither bus is enabled.
    pub fn next_tx(&mut self) -> Option<(Bus, &[u8])> {
        if let Some(bus) = self.requester.sending_bus() {
            return Some((bus, self.requester.frame()));
        }
        if self.egress.is_empty() {
            return self.next_request_tx();
        }
        let enabled = PerBus {
            a: self.buses.a.enabled(),
            b: self.buses.b.enabled(),
        };
        let head = self.egress.peek()?;
        let (assigned, current, arrival) = (head.assigned, head.bus, head.arrival);
        let bus = if !assigned {
            if !enabled.a && !enabled.b {
                return None;
            }
            let preferred = self.response_preference(arrival);
            self.choose_tx_bus(preferred)
        } else if *enabled.get(current) {
            current
        } else {
            let other = current.other();
            if !*enabled.get(other) {
                return None;
            }
            self.last_tx_bus = other;
            other
        };
        let frame = self.egress.peek_mut()?;
        frame.bus = bus;
        frame.assigned = true;
        let frame = &*frame;
        Some((frame.bus, frame.as_bytes()))
    }

    /// next_request_tx -- the requester's frame and its bus when a transmission is due. A request
    /// has no arrival bus: it prefers the bus other than its own latest transmission (Rules
    /// 4.1.1-6 and 4.1.1-7), so a response sent during a backoff does not change the retry bus;
    /// the first transmission of a request prefers the bus other than the latest frame's. The
    /// other enabled bus is taken when the preferred one is disabled (`choose_tx_bus`). None when
    /// nothing is due or no bus is enabled.
    fn next_request_tx(&mut self) -> Option<(Bus, &[u8])> {
        if !REQUESTS_BUILT
            || !self.requester.due()
            || !(self.buses.a.enabled() || self.buses.b.enabled())
        {
            return None;
        }
        let preferred = self
            .requester
            .last_bus()
            .map_or_else(|| self.last_tx_bus.other(), Bus::other);
        let bus = self.choose_tx_bus(preferred);
        self.requester.start_tx(bus);
        Some((bus, self.requester.frame()))
    }

    /// on_request_tx_result -- books the outcome of a request transmission on `bus`: the bus
    /// counters and the requester (success starts the T3 wait; a failure is retransmitted on the
    /// other bus after T6, Rule 4.1.1-7, when it is enabled, else at the next timeout).
    fn on_request_tx_result(
        &mut self,
        bus: Bus,
        outcome: TxOutcome,
        now_ms: u32,
        trace: &mut dyn Trace,
    ) {
        let ok = outcome == TxOutcome::Sent;
        let counters = self.counters.get_mut(bus);
        counters.tx_requests = counters.tx_requests.saturating_add(1);
        if ok {
            counters.tx_ok = counters.tx_ok.saturating_add(1);
            counters.tx_fail_streak = 0;
        } else {
            counters.tx_fail = counters.tx_fail.saturating_add(1);
            counters.tx_fail_streak = counters.tx_fail_streak.saturating_add(1);
        }
        let other_enabled = self.buses.get(bus.other()).enabled();
        let Some(sent) = self.requester.tx_done(ok, now_ms, other_enabled) else {
            return;
        };
        if !TRACE_ON {
            return;
        }
        let name = bus.name();
        let who = RequestWho(sent.dest_addr8);
        let what = RequestWhat {
            netfn: sent.netfn,
            cmd: sent.cmd,
            seq: sent.seq,
        };
        let n = sent.transmissions;
        match (outcome, n) {
            (TxOutcome::Sent, 1) => {
                trace.line(format_args!("IPMB-{name} request {who}: {what} sent"));
            }
            (TxOutcome::Sent, _) => trace.line(format_args!(
                "IPMB-{name} request {who}: {what} retry {} of {}",
                n - 1,
                crate::requester::RETRIES
            )),
            (TxOutcome::Failed(fault), _) => trace.line(format_args!(
                "IPMB-{name} request {who}: {what} not delivered, {} (transmission {n} of {})",
                fault.meaning(sent.dest_addr8),
                crate::requester::MAX_TRANSMISSIONS
            )),
        }
    }

    /// on_tx_result -- the driver reports the outcome of the frame `next_tx` handed out on `bus`,
    /// at `now_ms` (the end of the transmission). `outcome` is a `TxOutcome`, or a `bool` (true
    /// sent, false an unspecified failure).
    /// A request frame handed out by `next_tx` is booked by `on_request_tx_result`. Ok: the frame
    /// leaves the queue, and counts as a late response when `now_ms` is more than
    /// RESPONSE_DEADLINE_MS after the request arrived. Failure: the other bus is tried (Rule
    /// 4.1.1-7) when it is enabled and the frame has been resent fewer than TX_RETRIES times;
    /// otherwise the frame is dropped and counted (the requester retries and the duplicate cache
    /// replays the response). Err(NoTxInFlight) (nothing handed out) or Err(TxBusMismatch) leave
    /// everything unchanged.
    pub fn on_tx_result(
        &mut self,
        bus: Bus,
        outcome: impl Into<TxOutcome>,
        now_ms: u32,
        trace: &mut dyn Trace,
    ) -> Result<(), LinkError> {
        let outcome = outcome.into();
        if let Some(sent_on) = self.requester.sending_bus() {
            if sent_on != bus {
                return Err(LinkError::TxBusMismatch);
            }
            self.on_request_tx_result(bus, outcome, now_ms, trace);
            return Ok(());
        }
        let head = self.egress.peek().ok_or(LinkError::NoTxInFlight)?;
        if !head.assigned {
            return Err(LinkError::NoTxInFlight);
        }
        if head.bus != bus {
            return Err(LinkError::TxBusMismatch);
        }
        let resends = head.resends();
        let arrival_ms = head.arrival_ms;
        let (dest, seq) =
            ipmb::parse(head.as_bytes()).map_or((0, 0), |f| (f.header.dest, f.header.seq));
        let counters = self.counters.get_mut(bus);
        let TxOutcome::Failed(fault) = outcome else {
            counters.tx_ok = counters.tx_ok.saturating_add(1);
            counters.tx_fail_streak = 0;
            if now_ms.wrapping_sub(arrival_ms) > RESPONSE_DEADLINE_MS {
                self.late_responses = self.late_responses.saturating_add(1);
            }
            self.egress.pop();
            return Ok(());
        };
        counters.tx_fail = counters.tx_fail.saturating_add(1);
        counters.tx_fail_streak = counters.tx_fail_streak.saturating_add(1);
        let other = bus.other();
        if resends < TX_RETRIES && self.buses.get(other).enabled() {
            if let Some(head) = self.egress.peek_mut() {
                head.resend_on(other);
            }
            let counters = self.counters.get_mut(other);
            counters.resends = counters.resends.saturating_add(1);
            self.last_tx_bus = other;
            if TRACE_ON {
                trace.line(format_args!(
                    "IPMB-{} response to {dest:02X}h seq {seq:02X}h not delivered, {}; resend {} of {TX_RETRIES} on IPMB-{}",
                    bus.name(),
                    fault.meaning(dest),
                    resends + 1,
                    other.name()
                ));
            }
        } else {
            self.egress.pop();
            let counters = self.counters.get_mut(bus);
            counters.tx_dropped = counters.tx_dropped.saturating_add(1);
            if TRACE_ON {
                trace.line(format_args!(
                    "IPMB-{} response to {dest:02X}h seq {seq:02X}h not delivered, {}; dropped after {resends} resends",
                    bus.name(),
                    fault.meaning(dest)
                ));
            }
        }
        Ok(())
    }

    // ---- bus state ----

    /// set_ipmb_state -- applies a Set IPMB State request through `plan_ipmb_state`, the one
    /// rule: Err(BusNotFitted) or Err(IsolatesBothBuses) leave the state unchanged. The state
    /// changes only here and in `set_local_status` (Rule 9.3-10). `poll` calls it for the change
    /// the Set IPMB State handler left in `Context::pending_ipmb`.
    pub fn set_ipmb_state(
        &mut self,
        a: Option<BusMode>,
        b: Option<BusMode>,
    ) -> Result<(), LinkError> {
        self.buses = plan_ipmb_state(&self.buses, a, b)?;
        Ok(())
    }

    /// set_ipmb_speed -- applies the speed part of a Set IPMB State request through
    /// `plan_ipmb_speed`; Err(BusNotFitted) leaves the state unchanged. `poll` calls it for the
    /// change the handler left in `Context::pending_ipmb`; the owner of the master driver reads
    /// `bus_state(bus).speed` and applies it between transfers.
    pub fn set_ipmb_speed(
        &mut self,
        a: Option<BusSpeed>,
        b: Option<BusSpeed>,
    ) -> Result<(), LinkError> {
        self.buses = plan_ipmb_speed(&self.buses, a, b)?;
        Ok(())
    }

    /// view -- the read-only view handlers get for one dispatch.
    pub fn view(&self) -> LinkView {
        LinkView {
            config: self.config,
            buses: self.buses,
        }
    }

    /// set_local_status -- records a persistent local fault of `bus` (or its removal with
    /// `LocalStatus::None`); a faulted bus is not enabled (Rule 9.3-10). The Phase 6 driver
    /// decides when a fault is persistent, from the `counters`.
    pub fn set_local_status(&mut self, bus: Bus, status: LocalStatus) {
        self.buses.get_mut(bus).status = status;
    }

    // ---- readers ----

    /// bus_state -- state of one bus.
    pub fn bus_state(&self, bus: Bus) -> BusState {
        *self.buses.get(bus)
    }

    /// counters -- the counters of one bus.
    pub fn counters(&self, bus: Bus) -> &BusCounters {
        self.counters.get(bus)
    }

    /// config -- the build configuration.
    pub const fn config(&self) -> &LinkConfig {
        &self.config
    }

    /// own_address -- the 8-bit IPMB address requests must carry.
    pub const fn own_address(&self) -> u8 {
        self.own_addr
    }

    /// evictions -- live duplicate-cache entries replaced because the cache was full.
    pub const fn evictions(&self) -> u32 {
        self.cache.evictions()
    }

    /// late_responses -- responses whose transmission completed more than T5max
    /// (RESPONSE_DEADLINE_MS) after the request STOP.
    pub const fn late_responses(&self) -> u32 {
        self.late_responses
    }

    /// dispatch_errors -- requests that produced no queued response.
    pub const fn dispatch_errors(&self) -> u32 {
        self.dispatch_errors
    }

    /// egress_empty -- no response waits for transmission (the firmware also needs this for a
    /// quiet bus before a flash erase).
    pub const fn egress_empty(&self) -> bool {
        self.egress.is_empty()
    }

    /// request_in_flight -- the link holds a request it accepted and has not answered yet (in the
    /// ingress FIFO), or a response it has not transmitted yet (on the egress queue). The firmware
    /// defers a flash erase while this is true: the erase takes both slaves off the bus, and the
    /// Chassis Manager would lose the answer it waits for.
    pub const fn request_in_flight(&self) -> bool {
        !self.ingress.is_empty() || !self.egress.is_empty()
    }

    /// egress_dropped -- responses the egress queue rejected because it was full.
    pub const fn egress_dropped(&self) -> u32 {
        self.egress.dropped()
    }

    /// sensor_data -- both buses as the IPMB Link sensor reports them (`sensor::encode_link`).
    pub fn sensor_data(&self) -> LinkSensor {
        self.buses.sensor_data()
    }
}

impl BusCounters {
    /// ZERO -- all counters at 0 (const form of `Default`).
    const ZERO: BusCounters = BusCounters {
        rx_ok: 0,
        rx_errors: RxErrors {
            too_short: 0,
            too_long: 0,
            header_checksum: 0,
            data_checksum: 0,
            invalid_address: 0,
            other: 0,
        },
        not_for_us: 0,
        rx_response: 0,
        rx_answers: 0,
        bus_unavailable: 0,
        ingress_full: 0,
        tx_ok: 0,
        tx_requests: 0,
        tx_fail: 0,
        tx_fail_streak: 0,
        tx_dropped: 0,
        resends: 0,
        replays: 0,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addr;
    use crate::commands::cmd;
    use crate::dispatch::cc;
    use crate::ext::NoExtension;
    use crate::ipmb::{Frame, Header, NETFN_STORAGE};
    use crate::sensor::encode_link;
    use crate::trace::{NoTrace, RecordingTrace};
    use std::cell::Cell;

    const OWN: u8 = 0x82;
    const CM: u8 = 0x20;
    const COUNT_CMD: u8 = 0x70;

    thread_local! {
        /// Handler runs on this test's thread.
        static RUNS: Cell<u8> = const { Cell::new(0) };
    }

    /// Counts its runs and returns the count, so a replay shows the old count again.
    fn counting_handler(_c: &mut Context, _r: &Frame, out: &mut [u8]) -> Result<usize, u8> {
        let runs = RUNS.with(|n| {
            n.set(n.get() + 1);
            n.get()
        });
        *out.first_mut().ok_or(cc::UNSPECIFIED)? = runs;
        Ok(1)
    }

    fn runs() -> u8 {
        RUNS.with(Cell::get)
    }

    fn table() -> [Command; 1] {
        [cmd(
            NETFN_STORAGE,
            COUNT_CMD,
            "Count",
            "test",
            counting_handler,
        )]
    }

    fn ctx() -> Context {
        Context::new(addr::resolve(0x7F))
    }

    fn link_with(b_fitted: bool, tx_policy: TxPolicy) -> Link {
        let addr = addr::resolve(0x7F);
        assert_eq!(addr.ipmb_address, OWN);
        Link::new(
            LinkConfig {
                b_fitted,
                tx_policy,
                requests_allowed: false,
            },
            &addr,
        )
    }

    /// Both buses fitted and enabled, ALTERNATE.
    fn both_buses() -> Link {
        let mut link = link_with(true, TxPolicy::Alternate);
        link.set_ipmb_state(None, Some(BusMode::LocalControl))
            .unwrap();
        link
    }

    /// request_to_lun -- an encoded request from the Chassis Manager to `dest`, responder LUN `lun`.
    fn request_to_lun(dest: u8, lun: u8, netfn: u8, cmd: u8, seq: u8, data: &[u8]) -> Vec<u8> {
        let header = Header {
            dest,
            netfn,
            dest_lun: lun,
            src: CM,
            seq,
            src_lun: 0,
            cmd,
        };
        let mut buf = [0u8; IPMB_MAX_LEN];
        let n = ipmb::encode_request(&header, data, &mut buf).unwrap();
        buf[..n].to_vec()
    }

    fn request(dest: u8, netfn: u8, cmd: u8, seq: u8, data: &[u8]) -> Vec<u8> {
        request_to_lun(dest, 0, netfn, cmd, seq, data)
    }

    fn count_request(seq: u8, data: &[u8]) -> Vec<u8> {
        request(OWN, NETFN_STORAGE, COUNT_CMD, seq, data)
    }

    /// Pops every queued response as (bus, bytes), reporting success.
    fn drain(link: &mut Link) -> Vec<(Bus, Vec<u8>)> {
        let mut sent = Vec::new();
        while let Some((bus, bytes)) = link.next_tx() {
            sent.push((bus, bytes.to_vec()));
            link.on_tx_result(bus, true, 0, &mut NoTrace).unwrap();
        }
        sent
    }

    fn poll(link: &mut Link, now_ms: u32) -> usize {
        link.poll_with(&table(), &mut NoExtension, &mut ctx(), now_ms, &mut NoTrace)
    }

    #[test]
    fn power_up_state_is_a_on_b_off() {
        let fitted = link_with(true, TxPolicy::Alternate);
        let a = fitted.bus_state(Bus::A);
        let b = fitted.bus_state(Bus::B);
        assert_eq!(
            (a.fitted, a.mode, a.enabled()),
            (true, BusMode::LocalControl, true)
        );
        assert_eq!(
            (b.fitted, b.mode, b.enabled()),
            (true, BusMode::Isolated, false)
        );
        let bench = link_with(false, TxPolicy::Alternate);
        assert!(!bench.bus_state(Bus::B).fitted);
        assert!(!bench.bus_state(Bus::B).enabled());
        assert_eq!(fitted.own_address(), OWN);
    }

    #[test]
    fn default_config_follows_defs() {
        let c = LinkConfig::default();
        assert_eq!(c.b_fitted, ipmc_defs::IPMB_BUS_B_ENABLE);
        assert_eq!(c.tx_policy, ipmc_defs::IPMB_TX_POLICY);
    }

    #[test]
    fn disabled_and_unfitted_buses_drop_frames() {
        // Fitted but disabled at power-up.
        let mut link = link_with(true, TxPolicy::Alternate);
        let frame = count_request(1, &[]);
        assert_eq!(
            link.on_frame(Bus::B, &frame, 0),
            Err(FrameDrop::BusUnavailable)
        );
        assert_eq!(link.counters(Bus::B).bus_unavailable, 1);
        // Not fitted: stays unavailable, and Local Control cannot be requested.
        let mut bench = link_with(false, TxPolicy::Alternate);
        assert_eq!(
            bench.on_frame(Bus::B, &frame, 0),
            Err(FrameDrop::BusUnavailable)
        );
        assert_eq!(
            bench.set_ipmb_state(None, Some(BusMode::LocalControl)),
            Err(LinkError::BusNotFitted)
        );
        assert_eq!(bench.on_frame(Bus::A, &frame, 0), Ok(()));
        assert_eq!(poll(&mut bench, 0), 1);
    }

    #[test]
    fn broadcast_get_device_id_is_stripped_and_answered_with_own_address() {
        let mut link = link_with(false, TxPolicy::Alternate);
        // IPMI v2.0 20.9: 00h, then a normal frame whose rsSA is the address being probed. The
        // probes of other addresses are not for us, and only ours is answered.
        for probed in [0x20, 0x84, 0xBE] {
            let mut other = vec![GENERAL_CALL];
            other.extend(request(probed, NETFN_APP, CMD_GET_DEVICE_ID, 8, &[]));
            assert_eq!(link.on_frame(Bus::A, &other, 0), Err(FrameDrop::NotForUs));
        }
        assert_eq!(link.counters(Bus::A).not_for_us, 3);
        let mut frame = vec![GENERAL_CALL];
        frame.extend(request(OWN, NETFN_APP, CMD_GET_DEVICE_ID, 9, &[]));
        assert_eq!(link.on_frame(Bus::A, &frame, 0), Ok(()));
        assert_eq!(poll_real(&mut link, 0), 1);
        let sent = drain(&mut link);
        let [(Bus::A, bytes)] = sent.as_slice() else {
            panic!("one response on A: {sent:?}");
        };
        let rsp = ipmb::parse(bytes).unwrap();
        assert_eq!(
            (rsp.header.dest, rsp.header.src, rsp.header.seq),
            (CM, OWN, 9)
        );
        assert_eq!(rsp.completion_code, Some(cc::OK));
        assert_eq!(rsp.data.len(), crate::cmd::app::GET_DEVICE_ID_LEN);
    }

    /// poll over the real COMMANDS table.
    fn poll_real(link: &mut Link, now_ms: u32) -> usize {
        link.poll(&mut ctx(), &mut NoExtension, now_ms, &mut NoTrace)
    }

    #[test]
    fn broadcast_other_than_get_device_id_is_dropped() {
        let mut link = link_with(false, TxPolicy::Alternate);
        let mut frame = vec![GENERAL_CALL];
        frame.extend(request(OWN, NETFN_APP, 0x04, 1, &[]));
        assert_eq!(link.on_frame(Bus::A, &frame, 0), Err(FrameDrop::NotForUs));
        assert_eq!(link.counters(Bus::A).not_for_us, 1);
    }

    #[test]
    fn unicast_get_device_id_to_another_address_is_not_for_us() {
        let mut link = link_with(false, TxPolicy::Alternate);
        let frame = request(0x84, NETFN_APP, CMD_GET_DEVICE_ID, 1, &[]);
        assert_eq!(link.on_frame(Bus::A, &frame, 0), Err(FrameDrop::NotForUs));
        assert_eq!(link.counters(Bus::A).not_for_us, 1);
        assert_eq!(link.counters(Bus::A).rx_ok, 0);
    }

    /// response_to -- the empty OK response to the request frame `req`, with sequence number `seq`
    /// (chk2 recomputed by the encoder).
    fn response_to(req: &[u8], seq: u8) -> Vec<u8> {
        let mut header = ipmb::response_header_for(&ipmb::parse(req).unwrap()).unwrap();
        header.seq = seq;
        let mut buf = [0u8; IPMB_MAX_LEN];
        let n = ipmb::encode_response(&header, 0, &[], &mut buf).unwrap();
        buf[..n].to_vec()
    }

    #[test]
    fn responses_are_dropped_and_counted() {
        let mut link = link_with(false, TxPolicy::Alternate);
        // The response to a request from the CM, addressed to the CM.
        let req = request(OWN, NETFN_APP, CMD_GET_DEVICE_ID, 1, &[]);
        assert_eq!(
            link.on_frame(Bus::A, &response_to(&req, 1), 0),
            Err(FrameDrop::Response)
        );
        assert_eq!(link.counters(Bus::A).rx_response, 1);
        assert_eq!(poll_real(&mut link, 0), 0);
    }

    #[test]
    fn parse_errors_are_counted_by_kind() {
        let mut link = link_with(false, TxPolicy::Alternate);
        let good = count_request(1, &[1, 2]);
        let mut bad_chk1 = good.clone();
        bad_chk1[2] ^= 0xFF;
        let mut bad_chk2 = good.clone();
        let last = bad_chk2.len() - 1;
        bad_chk2[last] ^= 0xFF;
        let mut odd_address = good.clone();
        odd_address[0] |= 1;
        odd_address[2] = ipmb::checksum(&odd_address[..2]);
        let too_long = [0x82u8; IPMB_MAX_LEN + 1];
        for frame in [
            &bad_chk1[..],
            &bad_chk2[..],
            &odd_address[..],
            &too_long[..],
            &good[..3],
        ] {
            assert!(matches!(
                link.on_frame(Bus::A, frame, 0),
                Err(FrameDrop::Parse(_))
            ));
        }
        let e = link.counters(Bus::A).rx_errors;
        assert_eq!(
            (
                e.header_checksum,
                e.data_checksum,
                e.invalid_address,
                e.too_long,
                e.too_short
            ),
            (1, 1, 1, 1, 1)
        );
        assert_eq!((e.other, e.total()), (0, 5));
        assert_eq!(link.counters(Bus::B).rx_errors.total(), 0);
        assert_eq!(link.counters(Bus::A).rx_ok, 0);
    }

    #[test]
    fn full_ingress_drops_the_new_frame_and_keeps_the_queued_ones() {
        let mut link = link_with(false, TxPolicy::Alternate);
        for seq in 0..INGRESS_DEPTH as u8 {
            assert_eq!(link.on_frame(Bus::A, &count_request(seq, &[]), 0), Ok(()));
        }
        let extra = count_request(60, &[]);
        assert_eq!(
            link.on_frame(Bus::A, &extra, 0),
            Err(FrameDrop::IngressFull)
        );
        assert_eq!(link.counters(Bus::A).ingress_full, 1);
        assert_eq!(link.counters(Bus::A).rx_ok, INGRESS_DEPTH as u32);
        // The queue is smaller than the ingress FIFO, so drain between polls: all seqs in order.
        let mut seqs = Vec::new();
        for _ in 0..INGRESS_DEPTH {
            poll(&mut link, 0);
            for (_, bytes) in drain(&mut link) {
                seqs.push(ipmb::parse(&bytes).unwrap().header.seq);
            }
        }
        assert_eq!(seqs, (0..INGRESS_DEPTH as u8).collect::<Vec<_>>());
    }

    #[test]
    fn requests_from_both_buses_are_answered_in_arrival_order() {
        let mut link = link_with(true, TxPolicy::Arrival);
        link.set_ipmb_state(None, Some(BusMode::LocalControl))
            .unwrap();
        let order = [(Bus::A, 1u8), (Bus::B, 2), (Bus::B, 3), (Bus::A, 4)];
        for (i, (bus, seq)) in order.iter().enumerate() {
            link.on_frame(*bus, &count_request(*seq, &[]), i as u32)
                .unwrap();
        }
        assert_eq!(poll(&mut link, 10), 4);
        let sent: Vec<(Bus, u8)> = drain(&mut link)
            .iter()
            .map(|(bus, b)| (*bus, ipmb::parse(b).unwrap().header.seq))
            .collect();
        assert_eq!(sent, order);
        assert_eq!(link.counters(Bus::A).rx_ok, 2);
        assert_eq!(link.counters(Bus::B).rx_ok, 2);
    }

    #[test]
    fn egress_empty_follows_the_response_queue() {
        let mut link = link_with(false, TxPolicy::Alternate);
        assert!(link.egress_empty());
        assert!(!link.request_in_flight());
        link.on_frame(Bus::A, &count_request(1, &[]), 0).unwrap();
        assert!(
            link.egress_empty(),
            "a request in the ingress FIFO has no response yet"
        );
        // request_in_flight: accepted, not answered yet ...
        assert!(link.request_in_flight());
        assert_eq!(poll(&mut link, 0), 1);
        assert!(!link.egress_empty());
        // ... answered, not transmitted yet ...
        assert!(link.request_in_flight());
        drain(&mut link);
        assert!(link.egress_empty());
        // ... transmitted.
        assert!(!link.request_in_flight());
    }

    #[test]
    fn poll_leaves_requests_queued_while_the_egress_queue_is_full() {
        let mut link = link_with(false, TxPolicy::Alternate);
        for seq in 0..6 {
            link.on_frame(Bus::A, &count_request(seq, &[]), 0).unwrap();
        }
        assert_eq!(poll(&mut link, 0), crate::queue::TXQ_DEPTH);
        assert_eq!(link.egress_dropped(), 0);
        drain(&mut link);
        assert_eq!(poll(&mut link, 0), 6 - crate::queue::TXQ_DEPTH);
    }

    /// buses_answered -- one Count request (sequence = its index) arrives on each of `arrivals`
    /// at time 0; returns the bus each response goes out on, in order.
    fn buses_answered(link: &mut Link, arrivals: &[Bus]) -> Vec<Bus> {
        for (seq, &bus) in arrivals.iter().enumerate() {
            link.on_frame(bus, &count_request(seq as u8, &[]), 0)
                .unwrap();
        }
        poll(link, 0);
        drain(link).iter().map(|(b, _)| *b).collect()
    }

    #[test]
    fn alternate_policy_alternates_buses_and_stays_on_a_alone() {
        let mut link = both_buses();
        assert_eq!(
            buses_answered(&mut link, &[Bus::A; 4]),
            [Bus::A, Bus::B, Bus::A, Bus::B]
        );
        let mut alone = link_with(true, TxPolicy::Alternate); // B fitted, still disabled
        assert_eq!(
            buses_answered(&mut alone, &[Bus::A; 3]),
            [Bus::A, Bus::A, Bus::A]
        );
    }

    #[test]
    fn arrival_policy_answers_on_the_request_bus() {
        let mut link = link_with(true, TxPolicy::Arrival);
        link.set_ipmb_state(None, Some(BusMode::LocalControl))
            .unwrap();
        let arrivals = [Bus::B, Bus::B, Bus::A, Bus::B];
        assert_eq!(buses_answered(&mut link, &arrivals), arrivals);
        // The arrival bus is disabled by the time the response is chosen: the other bus is used.
        link.on_frame(Bus::B, &count_request(9, &[]), 0).unwrap();
        link.set_ipmb_state(Some(BusMode::LocalControl), Some(BusMode::Isolated))
            .unwrap();
        poll(&mut link, 0);
        assert_eq!(drain(&mut link)[0].0, Bus::A);
    }

    #[test]
    fn failed_transmit_is_resent_on_the_other_bus_then_dropped() {
        let mut link = both_buses();
        link.on_frame(Bus::A, &count_request(1, &[]), 0).unwrap();
        poll(&mut link, 0);
        let mut trace = RecordingTrace::default();
        let mut tried = Vec::new();
        while let Some((bus, _)) = link.next_tx() {
            tried.push(bus);
            link.on_tx_result(bus, false, 0, &mut trace).unwrap();
        }
        // First attempt plus TX_RETRIES resends, alternating A, B, A, B.
        let expected: Vec<Bus> = (0..=TX_RETRIES)
            .map(|i| if i % 2 == 0 { Bus::A } else { Bus::B })
            .collect();
        assert_eq!(tried, expected);
        let (a, b) = (link.counters(Bus::A), link.counters(Bus::B));
        assert_eq!(a.tx_fail + b.tx_fail, u32::from(TX_RETRIES) + 1);
        assert_eq!(a.resends + b.resends, u32::from(TX_RETRIES));
        assert_eq!((a.tx_ok, b.tx_ok), (0, 0));
        assert_eq!(a.tx_dropped + b.tx_dropped, 1);
        assert!(link.next_tx().is_none());
        assert_eq!(trace.lines.len(), usize::from(TX_RETRIES) + 1);
        assert_eq!(
            trace.lines[0],
            "IPMB-A response to 20h seq 01h not delivered, the transmission failed; resend 1 of 3 on IPMB-B"
        );
        assert!(
            trace.lines[3].contains("dropped after 3 resends"),
            "{:?}",
            trace.lines
        );
    }

    #[test]
    fn resend_that_succeeds_counts_ok_on_the_second_bus() {
        let mut link = both_buses();
        link.on_frame(Bus::A, &count_request(1, &[]), 0).unwrap();
        poll(&mut link, 0);
        let (bus, _) = link.next_tx().unwrap();
        assert_eq!(bus, Bus::A);
        link.on_tx_result(Bus::A, false, 0, &mut NoTrace).unwrap();
        let (bus, _) = link.next_tx().unwrap();
        assert_eq!(bus, Bus::B);
        link.on_tx_result(Bus::B, true, 0, &mut NoTrace).unwrap();
        assert!(link.next_tx().is_none());
        let (a, b) = (link.counters(Bus::A), link.counters(Bus::B));
        assert_eq!(
            (a.tx_fail, a.tx_fail_streak, b.tx_ok, b.resends),
            (1, 1, 1, 1)
        );
    }

    /// fail_first_transmit -- a Count request arrives on bus A at time 0 and the first attempt to
    /// answer it fails at `now_ms`.
    fn fail_first_transmit(link: &mut Link, now_ms: u32) {
        link.on_frame(Bus::A, &count_request(1, &[]), 0).unwrap();
        poll(link, 0);
        let (bus, _) = link.next_tx().unwrap();
        link.on_tx_result(bus, false, now_ms, &mut NoTrace).unwrap();
    }

    #[test]
    fn failed_transmit_with_one_bus_enabled_is_dropped_at_once() {
        let mut link = link_with(true, TxPolicy::Alternate);
        fail_first_transmit(&mut link, 0);
        assert!(link.next_tx().is_none());
        assert_eq!(
            (
                link.counters(Bus::A).tx_dropped,
                link.counters(Bus::A).resends
            ),
            (1, 0)
        );
    }

    #[test]
    fn tx_result_errors_leave_the_queue_alone() {
        let mut link = both_buses();
        assert_eq!(
            link.on_tx_result(Bus::A, true, 0, &mut NoTrace),
            Err(LinkError::NoTxInFlight)
        );
        link.on_frame(Bus::A, &count_request(1, &[]), 0).unwrap();
        poll(&mut link, 0);
        // A result before the frame was handed to a driver names nothing in flight.
        assert_eq!(
            link.on_tx_result(Bus::A, true, 0, &mut NoTrace),
            Err(LinkError::NoTxInFlight)
        );
        assert_eq!(link.next_tx().map(|(b, _)| b), Some(Bus::A));
        assert_eq!(
            link.on_tx_result(Bus::B, true, 0, &mut NoTrace),
            Err(LinkError::TxBusMismatch)
        );
        assert!(link.next_tx().is_some());
    }

    #[test]
    fn queued_frame_moves_when_its_bus_is_disabled() {
        let mut link = both_buses();
        link.on_frame(Bus::A, &count_request(1, &[]), 0).unwrap();
        poll(&mut link, 0);
        assert_eq!(link.next_tx().map(|(b, _)| b), Some(Bus::A)); // handed to the driver on A
        link.set_ipmb_state(Some(BusMode::Isolated), None).unwrap();
        assert_eq!(link.next_tx().map(|(b, _)| b), Some(Bus::B));
        // A persistent fault on B leaves nothing to send on.
        link.set_local_status(Bus::B, LocalStatus::ClockLowTimeout);
        assert!(link.next_tx().is_none());
    }

    #[test]
    fn set_ipmb_state_refuses_to_isolate_both_buses() {
        let mut link = both_buses();
        let before = link.sensor_data();
        assert_eq!(
            link.set_ipmb_state(Some(BusMode::Isolated), Some(BusMode::Isolated)),
            Err(LinkError::IsolatesBothBuses)
        );
        assert_eq!(link.sensor_data(), before);
        // One at a time is fine; the last enabled bus cannot be isolated.
        assert_eq!(link.set_ipmb_state(Some(BusMode::Isolated), None), Ok(()));
        assert_eq!(
            link.set_ipmb_state(None, Some(BusMode::Isolated)),
            Err(LinkError::IsolatesBothBuses)
        );
        // From power-up (B isolated) isolating A is refused too; no change is a no-op.
        let mut fresh = link_with(true, TxPolicy::Alternate);
        assert_eq!(
            fresh.set_ipmb_state(Some(BusMode::Isolated), None),
            Err(LinkError::IsolatesBothBuses)
        );
        assert_eq!(fresh.set_ipmb_state(None, None), Ok(()));
        assert!(fresh.bus_state(Bus::A).enabled());
        // Isolating A while enabling B in the same request is allowed.
        assert_eq!(
            fresh.set_ipmb_state(Some(BusMode::Isolated), Some(BusMode::LocalControl)),
            Ok(())
        );
        assert!(!fresh.bus_state(Bus::A).enabled() && fresh.bus_state(Bus::B).enabled());
    }

    #[test]
    fn local_fault_disables_a_bus_and_shows_in_the_sensor() {
        let mut link = both_buses();
        link.set_local_status(Bus::B, LocalStatus::ClockLowTimeout);
        assert!(!link.bus_state(Bus::B).enabled());
        let mut out = [0u8; 3];
        encode_link(&link.sensor_data(), &mut out).unwrap();
        // B override=local (bit 7), B status 5 (bits 6:4), A local (bit 3); A en / B dis.
        assert_eq!(out, [0xD8, 0x40, 0x02]);
        link.set_local_status(Bus::B, LocalStatus::None);
        encode_link(&link.sensor_data(), &mut out).unwrap();
        assert_eq!(out, [0x88, 0x40, 0x08]);
    }

    #[test]
    fn link_sensor_at_power_up_is_08_40_02() {
        let link = link_with(true, TxPolicy::Alternate);
        let mut out = [0u8; 3];
        encode_link(&link.sensor_data(), &mut out).unwrap();
        assert_eq!(out, [0x08, 0x40, 0x02]);
    }

    // ---- duplicate cache ----

    #[test]
    fn retry_on_the_other_bus_runs_the_handler_once_and_replays_identical_bytes() {
        let mut link = both_buses();
        let mut trace = RecordingTrace::default();
        let frame = count_request(0x14, &[5, 6]);
        link.on_frame(Bus::A, &frame, 1000).unwrap();
        link.poll_with(&table(), &mut NoExtension, &mut ctx(), 1000, &mut trace);
        // The CM times out after 250 ms and retries the same request on B.
        link.on_frame(Bus::B, &frame, 1250).unwrap();
        link.poll_with(&table(), &mut NoExtension, &mut ctx(), 1250, &mut trace);
        assert_eq!(runs(), 1);
        let sent = drain(&mut link);
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].1, sent[1].1, "identical response bytes");
        assert_eq!(ipmb::parse(&sent[0].1).unwrap().data, [1]);
        assert_eq!(link.counters(Bus::B).replays, 1);
        assert_eq!(link.counters(Bus::A).replays, 0);
        assert!(
            trace
                .lines
                .iter()
                .any(|l| l == "IPMB-B replay seq 14h FN(0Ah) CMD(70h) cached"),
            "{:?}",
            trace.lines
        );
    }

    #[test]
    fn the_same_request_after_the_expiry_runs_the_handler_again() {
        let mut link = both_buses();
        let frame = count_request(3, &[]);
        link.on_frame(Bus::A, &frame, 0).unwrap();
        poll(&mut link, 0);
        link.on_frame(Bus::B, &frame, crate::dupcache::DUP_WINDOW_MS - 1)
            .unwrap();
        poll(&mut link, crate::dupcache::DUP_WINDOW_MS - 1);
        assert_eq!(runs(), 1);
        drain(&mut link);
        link.on_frame(Bus::A, &frame, crate::dupcache::DUP_WINDOW_MS)
            .unwrap();
        poll(&mut link, crate::dupcache::DUP_WINDOW_MS);
        assert_eq!(runs(), 2);
        let sent = drain(&mut link);
        assert_eq!(ipmb::parse(&sent[0].1).unwrap().data, [2]);
    }

    #[test]
    fn same_sequence_with_different_data_is_not_a_duplicate() {
        let mut link = both_buses();
        link.on_frame(Bus::A, &count_request(7, &[1]), 0).unwrap();
        link.on_frame(Bus::B, &count_request(7, &[2]), 10).unwrap();
        poll(&mut link, 10);
        assert_eq!(runs(), 2);
        assert_eq!(link.counters(Bus::B).replays, 0);
    }

    #[test]
    fn one_request_more_than_the_cache_evicts_the_oldest() {
        // rqSeq is 6 bits (64 values), so distinct requests vary the sequence number and the
        // request data; the count follows IPMB_DUP_CACHE.
        const SEQ_VALUES: usize = 64;
        let distinct = |i: usize| count_request((i % SEQ_VALUES) as u8, &[(i / SEQ_VALUES) as u8]);
        let mut link = both_buses();
        let count = crate::dupcache::DUP_CACHE + 1;
        for i in 0..count {
            link.on_frame(Bus::A, &distinct(i), i as u32).unwrap();
            poll(&mut link, i as u32);
            drain(&mut link);
        }
        assert_eq!(usize::from(runs()), count);
        assert_eq!(link.evictions(), 1);
        // The newest entries still replay; the first one runs again.
        let later = count as u32 + 10;
        link.on_frame(Bus::B, &distinct(count - 1), later).unwrap();
        poll(&mut link, later);
        assert_eq!(usize::from(runs()), count);
        link.on_frame(Bus::B, &distinct(0), later + 1).unwrap();
        poll(&mut link, later + 1);
        assert_eq!(usize::from(runs()), count + 1);
    }

    #[test]
    fn late_responses_are_counted_at_transmit_completion() {
        let mut link = link_with(false, TxPolicy::Alternate);
        // Both requests arrived at 0 and were answered at once; only the transmit time differs.
        link.on_frame(Bus::A, &count_request(1, &[]), 0).unwrap();
        link.on_frame(Bus::A, &count_request(2, &[]), 0).unwrap();
        poll(&mut link, 0);
        let (bus, _) = link.next_tx().unwrap();
        link.on_tx_result(bus, true, RESPONSE_DEADLINE_MS, &mut NoTrace)
            .unwrap();
        assert_eq!(link.late_responses(), 0, "exactly T5max is in time");
        let (bus, _) = link.next_tx().unwrap();
        link.on_tx_result(bus, true, RESPONSE_DEADLINE_MS + 1, &mut NoTrace)
            .unwrap();
        assert_eq!(link.late_responses(), 1);
    }

    #[test]
    fn a_failed_attempt_is_not_late_until_a_transmission_completes() {
        let mut link = both_buses();
        fail_first_transmit(&mut link, 5_000);
        assert_eq!(link.late_responses(), 0);
        let (bus, _) = link.next_tx().unwrap();
        link.on_tx_result(bus, true, 5_001, &mut NoTrace).unwrap();
        assert_eq!(link.late_responses(), 1);
    }

    #[test]
    fn a_replayed_response_is_timed_from_the_retry() {
        let mut link = both_buses();
        let frame = count_request(1, &[]);
        link.on_frame(Bus::A, &frame, 1000).unwrap();
        poll(&mut link, 1000);
        let (bus, _) = link.next_tx().unwrap();
        link.on_tx_result(bus, true, 1000, &mut NoTrace).unwrap();
        // The retry arrives 250 ms later and is answered from the cache.
        link.on_frame(Bus::B, &frame, 1250).unwrap();
        poll(&mut link, 1250);
        let (bus, _) = link.next_tx().unwrap();
        link.on_tx_result(bus, true, 1250 + RESPONSE_DEADLINE_MS, &mut NoTrace)
            .unwrap();
        assert_eq!(link.late_responses(), 0);
        link.on_frame(Bus::A, &frame, 1300).unwrap();
        poll(&mut link, 1300);
        let (bus, _) = link.next_tx().unwrap();
        link.on_tx_result(bus, true, 1300 + RESPONSE_DEADLINE_MS + 1, &mut NoTrace)
            .unwrap();
        assert_eq!(link.late_responses(), 1);
    }

    // ---- transmit alternation follows the transmissions ----

    #[test]
    fn alternation_counts_a_resend_as_the_latest_transmission() {
        let mut link = both_buses();
        // Two responses are queued before anything is transmitted.
        link.on_frame(Bus::A, &count_request(1, &[]), 0).unwrap();
        link.on_frame(Bus::A, &count_request(2, &[]), 0).unwrap();
        assert_eq!(poll(&mut link, 0), 2);
        let seq_of = |bytes: &[u8]| ipmb::parse(bytes).unwrap().header.seq;
        let (bus, bytes) = link.next_tx().map(|(b, f)| (b, f.to_vec())).unwrap();
        assert_eq!((bus, seq_of(&bytes)), (Bus::A, 1));
        link.on_tx_result(Bus::A, false, 0, &mut NoTrace).unwrap();
        // The first response is resent on B ...
        let (bus, bytes) = link.next_tx().map(|(b, f)| (b, f.to_vec())).unwrap();
        assert_eq!((bus, seq_of(&bytes)), (Bus::B, 1));
        link.on_tx_result(Bus::B, true, 0, &mut NoTrace).unwrap();
        // ... so the second goes to A, not to B again.
        let (bus, bytes) = link.next_tx().map(|(b, f)| (b, f.to_vec())).unwrap();
        assert_eq!((bus, seq_of(&bytes)), (Bus::A, 2));
    }

    // ---- invalid hardware address ----

    #[test]
    fn an_invalid_hardware_address_isolates_both_buses() {
        // GA 1 with GAP also set: even parity (Rule 4.1.5.4.1-4), so the address is not usable.
        let bad = addr::decode(0x1E);
        assert!(!bad.parity_ok && bad.valid && !bad.usable());
        let mut link = Link::new(
            LinkConfig {
                b_fitted: true,
                tx_policy: TxPolicy::Alternate,
                requests_allowed: false,
            },
            &bad,
        );
        let frame = request(bad.ipmb_address, NETFN_STORAGE, COUNT_CMD, 1, &[]);
        for bus in [Bus::A, Bus::B] {
            assert_eq!(
                link.on_frame(bus, &frame, 0),
                Err(FrameDrop::BusUnavailable)
            );
            assert!(!link.bus_state(bus).enabled());
            assert_eq!(link.bus_state(bus).mode, BusMode::Isolated);
            assert_eq!(link.counters(bus).bus_unavailable, 1);
        }
        assert!(link.next_tx().is_none());
        // The IPMB Link sensor: neither bus in local control, neither enabled (state 01h).
        let mut out = [0u8; 3];
        encode_link(&link.sensor_data(), &mut out).unwrap();
        assert_eq!(out, [0x00, 0x40, 0x01]);
        // A usable address keeps the power-up state.
        let good = Link::new(LinkConfig::from_defs(), &addr::resolve(0x7F));
        assert!(good.bus_state(Bus::A).enabled());
    }

    // ---- duplicate key ----

    /// A request to this controller, responder LUN `lun`.
    fn lun_request(lun: u8, seq: u8, netfn: u8, cmd: u8, data: &[u8]) -> Vec<u8> {
        request_to_lun(OWN, lun, netfn, cmd, seq, data)
    }

    #[test]
    fn same_sum_data_with_the_same_sequence_is_not_a_duplicate() {
        // Write FRU Data 00 C0 00 01 02, then 00 C0 00 02 01: equal sums, same seq and
        // requester, inside the window. Both writes must run (offset C0h is past the image).
        let mut link = both_buses();
        let mut ctx = ctx();
        const WRITE_FRU_DATA: u8 = 0x12;
        for (bus, data) in [
            (Bus::A, [0x00, 0xC0, 0x00, 0x01, 0x02]),
            (Bus::B, [0x00, 0xC0, 0x00, 0x02, 0x01]),
        ] {
            let frame = lun_request(0, 9, NETFN_STORAGE, WRITE_FRU_DATA, &data);
            link.on_frame(bus, &frame, 100).unwrap();
            link.poll(&mut ctx, &mut NoExtension, 100, &mut NoTrace);
        }
        assert_eq!(
            ctx.fru.bytes()[0xC0..0xC2],
            [0x02, 0x01],
            "the second write ran"
        );
        assert_eq!(
            link.counters(Bus::A).replays + link.counters(Bus::B).replays,
            0
        );
        let sent = drain(&mut link);
        assert_eq!(sent.len(), 2);
        for (_, bytes) in sent {
            let response = ipmb::parse(&bytes).unwrap();
            assert_eq!(
                (response.completion_code, response.data),
                (Some(0x00), &[2][..])
            );
        }
    }

    #[test]
    fn a_request_to_another_lun_is_not_answered_from_the_cache_of_lun_zero() {
        // Get Device ID to rsLUN 1 is answered C2h; the same request to LUN 0 with the same
        // sequence number must get the real answer, not the cached C2h.
        let mut link = both_buses();
        let mut ctx = ctx();
        for (bus, lun) in [(Bus::A, 1u8), (Bus::B, 0)] {
            let frame = lun_request(lun, 4, NETFN_APP, CMD_GET_DEVICE_ID, &[]);
            link.on_frame(bus, &frame, 100).unwrap();
            link.poll(&mut ctx, &mut NoExtension, 100, &mut NoTrace);
        }
        let sent = drain(&mut link);
        let codes: Vec<(u8, usize)> = sent
            .iter()
            .map(|(_, bytes)| {
                let r = ipmb::parse(bytes).unwrap();
                (r.completion_code.unwrap(), r.data.len())
            })
            .collect();
        assert_eq!(
            codes,
            [
                (cc::INVALID_FOR_LUN, 0),
                (cc::OK, crate::cmd::app::GET_DEVICE_ID_LEN)
            ]
        );
        assert_eq!(link.counters(Bus::B).replays, 0);
    }

    #[test]
    fn dropped_frames_are_traced_by_poll_with_reason_and_bus() {
        let mut link = both_buses();
        let mut bad = count_request(1, &[]);
        bad[2] ^= 1;
        let _ = link.on_frame(Bus::B, &bad, 0);
        let _ = link.on_frame(Bus::A, &request(0x84, NETFN_APP, 1, 1, &[]), 0);
        let mut trace = RecordingTrace::default();
        link.poll_with(&table(), &mut NoExtension, &mut ctx(), 0, &mut trace);
        assert_eq!(
            trace.lines,
            [
                "IPMB-B drop: HeaderChecksum (7 bytes)",
                "IPMB-A drop: NotForUs (7 bytes)"
            ]
        );
    }

    // ---- the requester through the link ----

    use crate::requester::{self, Request, RequestError};

    const PLATFORM_EVENT_DATA: [u8; 7] = [0x04, 0xF0, 0x00, 0x6F, 0xA1, 0x00, 0x00];
    const PLATFORM_EVENT_FRAME: [u8; 14] = [
        0x20, 0x10, 0xD0, 0x82, 0x00, 0x02, 0x04, 0xF0, 0x00, 0x6F, 0xA1, 0x00, 0x00, 0x78,
    ];

    fn platform_event() -> Request<'static> {
        Request {
            dest_addr8: CM,
            dest_lun: 0,
            netfn: crate::ipmb::NETFN_SE,
            cmd: 0x02,
            data: &PLATFORM_EVENT_DATA,
        }
    }

    /// requesting_link -- a link whose requester may send; both buses enabled when `both`.
    fn requesting_link(both: bool) -> Link {
        let addr = addr::resolve(0x7F);
        let mut link = Link::new(
            LinkConfig {
                b_fitted: both,
                tx_policy: TxPolicy::Alternate,
                requests_allowed: true,
            },
            &addr,
        );
        if both {
            link.set_ipmb_state(None, Some(BusMode::LocalControl))
                .unwrap();
        }
        link
    }

    /// next_frame -- `next_tx` as an owned (bus, bytes).
    fn next_frame(link: &mut Link) -> Option<(Bus, Vec<u8>)> {
        link.next_tx().map(|(bus, bytes)| (bus, bytes.to_vec()))
    }

    #[test]
    fn a_request_goes_out_and_its_response_ends_it() {
        let mut link = requesting_link(true);
        let mut trace = RecordingTrace::default();
        assert_eq!(link.requester.submit(&platform_event(), 0), Ok(0));
        let (bus, frame) = next_frame(&mut link).unwrap();
        assert_eq!((bus, frame.as_slice()), (Bus::A, &PLATFORM_EVENT_FRAME[..]));
        assert_eq!(
            next_frame(&mut link).unwrap().0,
            Bus::A,
            "same frame until reported"
        );
        link.on_tx_result(bus, true, 1, &mut trace).unwrap();
        assert!(next_frame(&mut link).is_none());
        let response = [0x82, 0x14, 0x6A, 0x20, 0x00, 0x02, 0x00, 0xDE];
        assert_eq!(link.on_frame(Bus::B, &response, 5), Ok(()));
        assert_eq!(link.counters(Bus::B).rx_answers, 1);
        assert_eq!(link.counters(Bus::B).rx_response, 0);
        assert_eq!(link.counters(Bus::A).tx_requests, 1);
        assert_eq!(link.counters(Bus::A).tx_ok, 1);
        link.poll(&mut ctx(), &mut NoExtension, 6, &mut trace);
        let done = link.requester.take_completion().unwrap();
        assert_eq!(done.seq, 0);
        assert_eq!(done.outcome.unwrap().completion_code, 0);
        assert_eq!(
            trace.lines,
            [
                "IPMB-A request to 20h (Chassis Manager): S/E Platform Event seq 00h sent",
                "request to 20h (Chassis Manager): S/E Platform Event seq 00h answered CC 00h"
            ]
        );
    }

    #[test]
    fn a_failed_request_transmission_names_destination_command_and_cause() {
        let mut link = requesting_link(false);
        let mut trace = RecordingTrace::default();
        link.requester.submit(&platform_event(), 0).unwrap();
        let (bus, _) = next_frame(&mut link).unwrap();
        link.on_tx_result(bus, TxOutcome::Failed(TxFault::AddressNack), 1, &mut trace)
            .unwrap();
        assert_eq!(
            trace.lines,
            ["IPMB-A request to 20h (Chassis Manager): S/E Platform Event seq 00h not delivered, \
              address 20h not acknowledged: no device answers at 20h (transmission 1 of 11)"]
        );
        assert_eq!(link.counters(Bus::A).tx_fail, 1);
    }

    #[test]
    fn a_request_without_a_response_ends_with_the_destination_and_the_count() {
        let mut link = requesting_link(false);
        let mut trace = RecordingTrace::default();
        link.requester.submit(&platform_event(), 0).unwrap();
        let mut now = 0;
        for _ in 0..crate::requester::MAX_TRANSMISSIONS {
            let (bus, _) = next_frame(&mut link).unwrap();
            link.on_tx_result(bus, true, now, &mut trace).unwrap();
            now += crate::requester::TIMEOUT_MS;
            link.poll(&mut ctx(), &mut NoExtension, now, &mut trace);
        }
        assert_eq!(
            trace.lines.last().map(String::as_str),
            Some(
                "request to 20h (Chassis Manager): S/E Platform Event seq 00h got no response \
                 after 11 transmissions"
            )
        );
    }

    /// local_get_device_id -- Get Device ID through `local_request` on `link`.
    fn local_get_device_id(link: &mut Link) -> (usize, [u8; IPMB_MAX_LEN]) {
        link.local_request(
            &mut ctx(),
            &mut NoExtension,
            NETFN_APP,
            CMD_GET_DEVICE_ID,
            &[],
            0,
            &mut NoTrace,
        )
        .unwrap()
    }

    #[test]
    fn local_request_returns_the_bytes_a_bus_request_would_queue() {
        let mut bus = link_with(false, TxPolicy::Alternate);
        let frame = request(OWN, NETFN_APP, CMD_GET_DEVICE_ID, 0, &[]);
        assert_eq!(bus.on_frame(Bus::A, &frame, 0), Ok(()));
        assert_eq!(poll_real(&mut bus, 0), 1);
        let queued = drain(&mut bus);
        let mut local = link_with(false, TxPolicy::Alternate);
        let (len, bytes) = local_get_device_id(&mut local);
        assert_eq!(queued.len(), 1);
        assert_eq!(&bytes[..len], queued[0].1.as_slice());
        // No queue, no cache entry, no counter moved by the local request.
        assert!(local.egress_empty());
        assert_eq!(local.duplicate_cache_len(), 0);
        assert_eq!(bus.duplicate_cache_len(), 1);
        assert_eq!(local.dispatch_errors(), 0);
        // The same request twice runs the handler twice (no cache lookup).
        let again = local_get_device_id(&mut local);
        assert_eq!(&again.1[..again.0], &bytes[..len]);
    }

    #[test]
    fn a_local_set_ipmb_state_changes_the_bus_state_as_a_bus_request_does() {
        use crate::dispatch::VSO_IDENTIFIER;
        let data = [VSO_IDENTIFIER, 0xFF, 0x01]; // IPMB-A no change, IPMB-B Local Control
        let mut local = link_with(true, TxPolicy::Alternate);
        assert!(!local.bus_state(Bus::B).enabled());
        let (len, bytes) = local
            .local_request(
                &mut ctx(),
                &mut NoExtension,
                crate::ipmb::NETFN_GROUP_EXT,
                0x09,
                &data,
                0,
                &mut NoTrace,
            )
            .unwrap();
        assert_eq!(
            ipmb::parse(&bytes[..len]).unwrap().completion_code,
            Some(cc::OK)
        );
        assert!(local.bus_state(Bus::B).enabled());
        // The same request over the bus gives the same bus state.
        let mut bus = link_with(true, TxPolicy::Alternate);
        let frame = request(OWN, crate::ipmb::NETFN_GROUP_EXT, 0x09, 0, &data);
        assert_eq!(bus.on_frame(Bus::A, &frame, 0), Ok(()));
        poll_real(&mut bus, 0);
        assert_eq!(bus.bus_state(Bus::B), local.bus_state(Bus::B));
    }

    #[test]
    fn a_local_request_too_long_for_a_frame_is_refused() {
        let mut link = link_with(false, TxPolicy::Alternate);
        let data = [0u8; IPMB_MAX_LEN];
        let result = link.local_request(
            &mut ctx(),
            &mut NoExtension,
            NETFN_APP,
            CMD_GET_DEVICE_ID,
            &data,
            0,
            &mut NoTrace,
        );
        assert_eq!(result.err(), Some(LocalError::TooLong));
    }

    #[test]
    fn clearing_the_duplicate_cache_makes_a_repeat_run_the_handler_again() {
        let mut link = link_with(false, TxPolicy::Alternate);
        let frame = count_request(1, &[]);
        let before = runs();
        for expected_runs in [1, 1] {
            assert_eq!(link.on_frame(Bus::A, &frame, 0), Ok(()));
            poll(&mut link, 0);
            drain(&mut link);
            assert_eq!(runs() - before, expected_runs, "a repeat is replayed");
        }
        assert_eq!(link.duplicate_cache_len(), 1);
        link.clear_duplicate_cache();
        assert_eq!(link.duplicate_cache_len(), 0);
        assert_eq!(link.on_frame(Bus::A, &frame, 0), Ok(()));
        poll(&mut link, 0);
        assert_eq!(
            runs() - before,
            2,
            "the cleared cache runs the handler again"
        );
    }

    #[test]
    fn a_response_that_matches_nothing_is_dropped_and_the_request_stays() {
        let mut link = requesting_link(false);
        link.requester.submit(&platform_event(), 0).unwrap();
        let (bus, _) = next_frame(&mut link).unwrap();
        link.on_tx_result(bus, true, 1, &mut NoTrace).unwrap();
        // The right response with the sequence number changed (chk2 recomputed by the encoder).
        assert_eq!(
            link.on_frame(Bus::A, &response_to(&PLATFORM_EVENT_FRAME, 1), 5),
            Err(FrameDrop::Response)
        );
        assert_eq!(link.counters(Bus::A).rx_response, 1);
        assert_eq!(link.counters(Bus::A).rx_answers, 0);
        assert!(link.requester.busy());
        assert!(link.requester.take_completion().is_none());
    }

    /// run_unanswered -- one request, never answered, every transmission succeeds; steps time in
    /// 1 ms and returns (time, bus, seq) of each transmission and the time the request ended.
    fn run_unanswered(both: bool) -> (Vec<(u32, Bus, u8)>, Option<u32>) {
        let mut link = requesting_link(both);
        link.requester.submit(&platform_event(), 0).unwrap();
        let (mut sent, mut ended) = (Vec::new(), None);
        for now in 0..=3 * requester::TIMEOUT_MS * u32::from(requester::MAX_TRANSMISSIONS) {
            poll_real(&mut link, now);
            while let Some((bus, bytes)) = next_frame(&mut link) {
                let seq = ipmb::parse(&bytes).unwrap().header.seq;
                sent.push((now, bus, seq));
                link.on_tx_result(bus, true, now, &mut NoTrace).unwrap();
            }
            if ended.is_none() && link.requester.take_completion().is_some() {
                ended = Some(now);
            }
        }
        (sent, ended)
    }

    #[test]
    fn an_unanswered_request_is_retried_with_its_sequence_then_ends() {
        let t3 = requester::TIMEOUT_MS;
        for both in [true, false] {
            let (sent, ended) = run_unanswered(both);
            // Alternating buses (Rule 4.1.1-6) with both enabled, else always A.
            let buses: Vec<Bus> = (0..requester::MAX_TRANSMISSIONS)
                .map(|i| if both && i % 2 == 1 { Bus::B } else { Bus::A })
                .collect();
            let expected: Vec<(u32, Bus, u8)> = buses
                .iter()
                .enumerate()
                .map(|(i, bus)| (i as u32 * t3, *bus, 0))
                .collect();
            assert_eq!(sent, expected, "both buses: {both}");
            // The last transmission waits one more timeout; no (2 + RETRIES)th goes out.
            assert_eq!(ended, Some(t3 * u32::from(requester::MAX_TRANSMISSIONS)));
        }
    }

    #[test]
    fn a_failed_request_transmission_is_retried_after_t6_on_the_other_enabled_bus() {
        let mut link = requesting_link(true);
        let mut trace = RecordingTrace::default();
        link.requester.submit(&platform_event(), 0).unwrap();
        let (bus, first) = next_frame(&mut link).unwrap();
        assert_eq!(bus, Bus::A);
        link.on_tx_result(bus, false, 0, &mut trace).unwrap();
        for now in 0..requester::T6_MIN_MS {
            poll_real(&mut link, now);
            assert!(next_frame(&mut link).is_none(), "T6 not over at {now} ms");
        }
        poll_real(&mut link, requester::T6_MIN_MS);
        let (bus, second) = next_frame(&mut link).unwrap();
        assert_eq!(
            (bus, &second),
            (Bus::B, &first),
            "same frame, other bus, after T6"
        );
        let second_at = requester::T6_MIN_MS;
        link.on_tx_result(bus, true, second_at, &mut trace).unwrap();
        assert_eq!(link.counters(Bus::A).tx_requests, 1);
        assert_eq!(link.counters(Bus::A).tx_fail, 1);
        assert_eq!(link.counters(Bus::B).tx_requests, 1);
        assert_eq!(link.counters(Bus::B).tx_ok, 1);
        assert_eq!(trace.lines.len(), 2);
        assert!(
            trace.lines[1].ends_with(&format!("retry 1 of {}", requester::RETRIES)),
            "{:?}",
            trace.lines
        );
        // The failed one counts as a transmission: 1 + RETRIES - 2 are left.
        let mut more = 0;
        for now in second_at + 1
            ..second_at + 4 * requester::TIMEOUT_MS * u32::from(requester::MAX_TRANSMISSIONS)
        {
            poll_real(&mut link, now);
            while let Some((bus, _)) = next_frame(&mut link) {
                more += 1;
                link.on_tx_result(bus, true, now, &mut NoTrace).unwrap();
            }
        }
        assert_eq!(more, usize::from(requester::MAX_TRANSMISSIONS) - 2);
    }

    #[test]
    fn a_response_sent_during_the_backoff_does_not_change_the_retry_bus() {
        let mut link = requesting_link(true);
        link.requester.submit(&platform_event(), 0).unwrap();
        let (bus, _) = next_frame(&mut link).unwrap();
        assert_eq!(bus, Bus::A);
        link.on_tx_result(bus, false, 0, &mut NoTrace).unwrap();
        // A Get Device ID request arrives; its response goes out on B during the T6 wait.
        link.on_frame(
            Bus::A,
            &request(OWN, NETFN_APP, CMD_GET_DEVICE_ID, 7, &[]),
            1,
        )
        .unwrap();
        poll_real(&mut link, 1);
        let (response_bus, response) = next_frame(&mut link).unwrap();
        assert_eq!(response_bus, Bus::B);
        assert_eq!(ipmb::parse(&response).unwrap().header.src, OWN);
        link.on_tx_result(response_bus, true, 2, &mut NoTrace)
            .unwrap();
        poll_real(&mut link, requester::T6_MIN_MS);
        let (retry_bus, retry) = next_frame(&mut link).unwrap();
        assert_eq!(retry, PLATFORM_EVENT_FRAME);
        assert_eq!(
            retry_bus,
            Bus::B,
            "the alternate of the failed request's bus"
        );
    }

    #[test]
    fn a_failed_request_transmission_with_one_bus_waits_for_the_timeout() {
        let mut link = requesting_link(false);
        link.requester.submit(&platform_event(), 0).unwrap();
        let (bus, _) = next_frame(&mut link).unwrap();
        link.on_tx_result(bus, false, 10, &mut NoTrace).unwrap();
        poll_real(&mut link, 10 + requester::TIMEOUT_MS - 1);
        assert!(next_frame(&mut link).is_none());
        poll_real(&mut link, 10 + requester::TIMEOUT_MS);
        assert_eq!(next_frame(&mut link).unwrap().0, Bus::A);
    }

    #[test]
    fn responses_go_out_before_a_due_request() {
        let mut link = requesting_link(true);
        link.requester.submit(&platform_event(), 0).unwrap();
        link.on_frame(
            Bus::A,
            &request(OWN, NETFN_APP, CMD_GET_DEVICE_ID, 7, &[]),
            0,
        )
        .unwrap();
        assert_eq!(poll_real(&mut link, 0), 1);
        let (bus, first) = next_frame(&mut link).unwrap();
        assert_eq!(
            ipmb::parse(&first).unwrap().header.src,
            OWN,
            "the response first"
        );
        link.on_tx_result(bus, true, 1, &mut NoTrace).unwrap();
        let (_, second) = next_frame(&mut link).unwrap();
        assert_eq!(second, PLATFORM_EVENT_FRAME);
    }

    #[test]
    fn a_tier_one_link_sends_no_request() {
        let mut link = link_with(true, TxPolicy::Alternate);
        assert_eq!(
            link.requester.submit(&platform_event(), 0),
            Err(RequestError::NotAllowed)
        );
        assert!(next_frame(&mut link).is_none());
        assert_eq!(
            LinkConfig::from_defs().requests_allowed,
            ipmc_defs::IPMC_TIER >= 2
        );
        if !ipmc_defs::TIER23_BUILT {
            assert!(!LinkConfig::from_defs().requests_allowed);
        }
    }

    /// Counts its poll calls and keeps the bus view it saw.
    #[derive(Default)]
    struct PollProbe {
        polls: u32,
        b_enabled_seen: Option<bool>,
    }

    impl Extension for PollProbe {
        fn command(
            &mut self,
            _ctx: &mut Context,
            _req: &Frame,
            _out: &mut [u8],
        ) -> Option<(&'static str, Result<usize, u8>)> {
            None
        }

        fn poll(
            &mut self,
            ctx: &mut Context,
            _requester: &mut Requester,
            _now_ms: u32,
            _trace: &mut dyn Trace,
        ) {
            self.polls += 1;
            self.b_enabled_seen = Some(ctx.link.buses.b.enabled());
        }
    }

    #[test]
    fn poll_calls_the_extension_once_with_a_refreshed_link_view() {
        let mut link = both_buses();
        let mut context = ctx();
        assert!(
            !context.link.buses.b.enabled(),
            "the context view starts stale"
        );
        let mut probe = PollProbe::default();
        link.poll(&mut context, &mut probe, 0, &mut NoTrace);
        assert_eq!((probe.polls, probe.b_enabled_seen), (1, Some(true)));
    }

    #[test]
    fn the_speed_rule_refuses_400_khz_on_a_bus_that_is_not_fitted() {
        let single = BusPair::power_up(false, true);
        let both = BusPair::power_up(true, true);
        // (pair, A, B, result): Standard is always accepted, Fast needs a fitted bus.
        let fast = Some(BusSpeed::Fast);
        let std = Some(BusSpeed::Standard);
        assert!(plan_ipmb_speed(&single, fast, None).is_ok());
        assert_eq!(
            plan_ipmb_speed(&single, None, fast),
            Err(LinkError::BusNotFitted)
        );
        assert!(plan_ipmb_speed(&single, std, std).is_ok());
        let next = plan_ipmb_speed(&both, fast, std).unwrap();
        assert_eq!(
            (next.a.speed, next.b.speed),
            (BusSpeed::Fast, BusSpeed::Standard)
        );
        assert_eq!(plan_ipmb_speed(&both, None, None), Ok(both));
        // All or nothing: the refused request leaves the view alone.
        assert!(plan_ipmb_speed(&single, fast, fast).is_err());
    }

    #[test]
    fn a_bus_powers_up_at_100_khz_and_the_speed_follows_set_ipmb_speed() {
        let mut link = link_with(true, TxPolicy::Alternate);
        assert_eq!(link.bus_state(Bus::A).speed, BusSpeed::Standard);
        assert_eq!(link.bus_state(Bus::B).speed, BusSpeed::Standard);
        assert_eq!(link.set_ipmb_speed(Some(BusSpeed::Fast), None), Ok(()));
        assert_eq!(link.bus_state(Bus::A).speed, BusSpeed::Fast);
        assert!(link.sensor_data().a.fast && !link.sensor_data().b.fast);
        assert_eq!(link.view().buses.a.speed, BusSpeed::Fast);
        // The state of a bus is independent of its speed.
        assert!(link.bus_state(Bus::A).enabled());
        assert_eq!(
            link.set_ipmb_speed(None, Some(BusSpeed::Fast)),
            Ok(()),
            "B is fitted here"
        );
        let mut single = link_with(false, TxPolicy::Alternate);
        assert_eq!(
            single.set_ipmb_speed(None, Some(BusSpeed::Fast)),
            Err(LinkError::BusNotFitted)
        );
    }

    #[test]
    fn a_restarted_link_has_the_power_up_state_and_keeps_the_request_sequence() {
        let mut link = link_with(true, TxPolicy::Alternate);
        link.set_ipmb_state(None, Some(BusMode::LocalControl))
            .unwrap();
        link.set_ipmb_speed(Some(BusSpeed::Fast), None).unwrap();
        let addr = crate::addr::resolve(0x7F);
        let fresh = link.restarted(&addr, 500);
        assert_eq!(fresh.config(), link.config());
        assert_eq!(fresh.own_address(), link.own_address());
        assert_eq!(
            fresh.bus_state(Bus::A).speed,
            BusSpeed::Standard,
            "Rule 4.1.1-4.3"
        );
        assert!(
            !fresh.bus_state(Bus::B).enabled(),
            "B isolated again (Rule 4.1.1-4)"
        );
        assert_eq!(fresh.counters(Bus::A), &BusCounters::default());
        assert_eq!(fresh.next_request_seq(), link.next_request_seq());
        assert_eq!(
            fresh.recent_request_seqs(500),
            link.recent_request_seqs(500)
        );
    }
}

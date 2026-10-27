// sensing.rs - firmware composition of the sensors crate and the SENSORS, POWER and SIM console
// groups.
//
// This module builds the backends from the ipmc-defs source switches (ADC1 and the board
// GPIO from ipmc-sensors, simulators beside them), owns the Monitor that samples and runs the
// power state machine, and holds the handlers of the three command groups. console.rs only
// registers the tables. The decisions live in ipmc-sensors and ipmc-core; this file moves
// values between them and the console.
//
// Trace lines go out through ipmc_core::trace::Trace, implemented by console::ConsoleTrace.
// Nothing here runs in an interrupt handler.

use core::fmt;

use ipmc_core::dispatch::Context as DispatchContext;
use ipmc_core::fru_state::PowerOnRefused;
use ipmc_core::link::BusPair;
use ipmc_core::sensor::{
    SensorSnapshot, READING_LEN_MAX, SENSOR_FRU_HEALTH, SENSOR_FRU_STATE, SENSOR_FRU_TEMPERATURE,
    SENSOR_FRU_VOLTAGE, SENSOR_PAYLOAD_TEST_RESULTS, SENSOR_PAYLOAD_TEST_STATUS,
    SENSOR_SYSTEM_IPMB_LINK,
};
use ipmc_menu::{on_off_text, optional, Args, CmdError, CmdResult};
use ipmc_sensors::analog::SPECS;
use ipmc_sensors::board_sensors::{sim_analog, INPUTS, RAILS};
use ipmc_sensors::drivers::sim::{SimAnalog, SimInputs, SimPower};
use ipmc_sensors::drivers::stm32f401::adc::{Adc, InitError};
use ipmc_sensors::drivers::stm32f401::gpio::BoardIo;
use ipmc_sensors::eval::faults;
use ipmc_sensors::eval::power_fsm::PowerError;
use ipmc_sensors::hal::{Channel, DigitalInputs, PowerSwitch, SourceError};
use ipmc_sensors::monitor::Monitor;
use ipmc_sensors::reset_pulse::PayloadResetPulse;
use ipmc_sensors::routing::{HardwareNeeds, RoutedAnalog, RoutedIo, Source};
use ipmc_sensors::trace::{Asserted, Level, Tenths};

use crate::bsp::systick;
use crate::console::{ConsoleTrace, Context};
use ipmc_core::dispatch::HandOffs;

// No bus is assigned to external sensor parts (the STM32's I2C1 and I2C3 are the IPMB buses), so
// the I2C and SPI source values are not accepted in a firmware build.
/// buildable -- true for the source values this firmware can serve: hardware or simulation.
const fn buildable(source: Source) -> bool {
    matches!(source, Source::Hw | Source::Sim)
}
const _: () = assert!(
    buildable(ipmc_defs::IPMC_ADC_INTERNAL_SOURCE)
        && buildable(ipmc_defs::IPMC_ADC_EXTERNAL_SOURCE)
        && buildable(ipmc_defs::IPMC_FAULT_SOURCE)
        && buildable(ipmc_defs::IPMC_POWER_SOURCE),
    "no bus assigned to external sensor parts: Source::I2c and Source::Spi cannot be selected"
);

/// hardware_needs -- which on-chip hardware the ipmc-defs source switches ask for. main.rs
/// starts only that: a simulated group claims and configures no pin.
pub const fn hardware_needs() -> HardwareNeeds {
    HardwareNeeds::of(
        ipmc_defs::IPMC_ADC_INTERNAL_SOURCE,
        ipmc_defs::IPMC_ADC_EXTERNAL_SOURCE,
        ipmc_defs::IPMC_FAULT_SOURCE,
        ipmc_defs::IPMC_POWER_SOURCE,
    )
}

/// Sensing -- everything the sensor commands and the main loop share.
pub struct Sensing {
    monitor: Monitor,
    analog: RoutedAnalog<Adc>,
    io: RoutedIo<BoardIo>,
    /// Set when the ADC refused to start; shown by SENSORS SHOW.
    adc_fault: Option<InitError>,
    /// A sample was taken that `take_fresh` has not handed on yet.
    fresh: bool,
    /// The timing of the payload reset pulse on PAYLOAD_RST_OUT_N (IPMC_PAYLOAD_RESET_PULSE_MS).
    reset_pulse: PayloadResetPulse,
}

impl Sensing {
    /// payload_power_on -- PWR_EN is asserted: the payload has power (`DEBUG BOOTLOADER` says so
    /// before it restarts the processor, which releases PWR_EN).
    pub fn payload_power_on(&self) -> bool {
        self.io.is_enabled()
    }

    /// new -- builds the routers over the hardware backends and the simulators at their
    /// ipmc-defs start values. `adc` is None when no group uses the ADC, `io` is None when no
    /// group uses the board pins; an ADC that refused to start is kept as a fault to show.
    pub fn new(adc: Option<Result<Adc, InitError>>, io: Option<BoardIo>) -> Self {
        let (adc, adc_fault) = match adc {
            Some(Ok(adc)) => (Some(adc), None),
            Some(Err(fault)) => (None, Some(fault)),
            None => (None, None),
        };
        Sensing {
            monitor: Monitor::new(
                ipmc_defs::IPMC_SENSOR_PERIOD_MS,
                ipmc_defs::IPMC_PGOOD_TIMEOUT_MS,
                Level::from_define(ipmc_defs::IPMC_SENSOR_TRACE),
            ),
            analog: RoutedAnalog::new(
                adc,
                sim_analog(),
                ipmc_defs::IPMC_ADC_INTERNAL_SOURCE,
                ipmc_defs::IPMC_ADC_EXTERNAL_SOURCE,
            ),
            io: RoutedIo::new(
                io,
                SimInputs::new(),
                SimPower::new(),
                ipmc_defs::IPMC_FAULT_SOURCE,
                ipmc_defs::IPMC_POWER_SOURCE,
            ),
            adc_fault,
            fresh: false,
            reset_pulse: PayloadResetPulse::new(),
        }
    }

    /// serve_hand_offs -- what the owner of the payload power and of the reset output does with
    /// the requests of the command handlers (`IpmbLink::take_hand_offs`), and the release of a
    /// running reset pulse. Called every main-loop pass.
    /// - A power request goes through `Monitor::apply_power_request` (On asks the FRU state
    ///   machine first, the one power-on check); a resample follows so the snapshot and the FRU
    ///   state see it.
    /// - A reset request asserts PAYLOAD_RST_OUT_N for IPMC_PAYLOAD_RESET_PULSE_MS
    ///   (`PayloadResetPulse`, non-blocking); a build whose power group is simulated has no
    ///   output and prints that.
    /// - A pulse whose time is up is released here.
    pub fn serve_hand_offs(&mut self, hand_offs: HandOffs) {
        let now = systick::millis();
        if let Some(request) = hand_offs.power {
            let done = self.monitor.apply_power_request(
                request,
                hand_offs.power_on_check,
                now,
                &mut self.io,
                &mut ConsoleTrace,
            );
            if done {
                self.sample_now();
            }
        }
        if hand_offs.payload_reset {
            let pulse_ms = ipmc_defs::IPMC_PAYLOAD_RESET_PULSE_MS;
            let started = match self.io.hw.as_mut() {
                Some(hw) => self.reset_pulse.pulse_payload_reset(hw, now, pulse_ms),
                None => Err(SourceError::Unavailable),
            };
            match started {
                Ok(()) => out!("Payload reset: PAYLOAD_RST_OUT_N low for {} ms\n", pulse_ms),
                Err(error) => out!("Payload reset: no output pulse: {}\n", error.name()),
            }
        }
        if let Some(hw) = self.io.hw.as_mut() {
            if let Some(Err(error)) = self.reset_pulse.service(hw, now) {
                out!("Payload reset: release failed: {}\n", error.name());
            }
        }
    }

    /// start -- requests payload power when ipmc-defs IPMC_POWER_AT_BOOT is true and
    /// `power_on_check` (`IpmbLink::power_on_check`) allows it; a refusal prints one line.
    pub fn start(&mut self, power_on_check: Result<(), PowerOnRefused>) {
        if !ipmc_defs::IPMC_POWER_AT_BOOT {
            return;
        }
        match power_on_check {
            // A fresh controller is Off with no fault, so the request cannot be refused.
            Ok(()) => {
                let _ = self
                    .monitor
                    .power_on(systick::millis(), &mut self.io, &mut ConsoleTrace);
            }
            Err(refused) => out!("Power at boot refused: {}\n", refused.name()),
        }
    }

    /// sample_with -- runs one monitor sampling operation (`op` gets the monitor, the time, the
    /// routers and the trace sink; it returns whether a sample was taken) and keeps the result
    /// for `take_fresh`.
    fn sample_with(
        &mut self,
        op: impl FnOnce(
            &mut Monitor,
            u32,
            &mut RoutedAnalog<Adc>,
            &mut RoutedIo<BoardIo>,
            &mut ConsoleTrace,
        ) -> bool,
    ) {
        self.fresh |= op(
            &mut self.monitor,
            systick::millis(),
            &mut self.analog,
            &mut self.io,
            &mut ConsoleTrace,
        );
    }

    /// service -- samples when the period has elapsed. Called from the main loop.
    pub fn service(&mut self) {
        self.sample_with(|monitor, now, analog, io, trace| monitor.service(now, analog, io, trace));
    }

    /// sample_now -- samples at once, so a console change shows in the next SENSORS SHOW.
    fn sample_now(&mut self) {
        self.sample_with(|monitor, now, analog, io, trace| {
            monitor.sample(now, analog, io, trace);
            true
        });
    }

    /// take_fresh -- the latest snapshot when a sample (periodic or taken by a console command)
    /// has not been handed on yet, else None. The main loop passes it to the IPMI handlers and
    /// the FRU state machine, so each sample is fed exactly once.
    pub fn take_fresh(&mut self) -> Option<SensorSnapshot> {
        let fresh = core::mem::take(&mut self.fresh);
        fresh.then(|| *self.monitor.snapshot())
    }
}

// ---- output helpers ----

/// SourceName -- "hardware" or "simulated" for a group line.
struct SourceName(Source);

impl fmt::Display for SourceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            Source::Hw => "hardware",
            Source::Sim => "simulated",
            Source::I2c => "external I2C (no bus assigned)",
            Source::Spi => "external SPI (no bus assigned)",
        })
    }
}

/// print_reading -- the Get Sensor Reading data bytes `Context::read_sensor` returns for sensor
/// `number` (the function Get Sensor Reading uses), with a one-line meaning.
fn print_reading(
    dispatch: &DispatchContext,
    buses: &BusPair,
    label: &str,
    number: u8,
    summary: &str,
) {
    let mut bytes = [0u8; READING_LEN_MAX];
    match dispatch.read_sensor(number, buses, &mut bytes) {
        Ok(len) => {
            out!(
                "  {} (sensor {}): {}\n    Get Sensor Reading data:",
                label,
                number,
                summary
            );
            for byte in bytes.iter().take(len) {
                out!(" {:02X}", byte);
            }
            out!("\n");
        }
        Err(e) => out!("  {} (sensor {}): read failed: {:?}\n", label, number, e),
    }
}

/// not_simulated -- prints that a group is not simulated; the command fails as reported.
fn not_simulated(group: &str, define: &str) -> CmdResult {
    out!(
        "Not simulated: the {} source is hardware (ipmc-defs {})\n",
        group,
        define
    );
    Err(CmdError::Reported)
}

/// rail_source -- the backend of rail `index` of the board table.
fn rail_source(sensing: &Sensing, index: usize) -> Source {
    match RAILS.get(index).map(|def| def.channel) {
        Some(Channel::Vdda) => sensing.analog.internal_source(),
        _ => sensing.analog.external_source(),
    }
}

// ---- SENSORS ----

/// cmd_sensors_show -- every rail, the die temperature, the inputs, the power state and the
/// three Tier-1 readings with their Get Sensor Reading data bytes.
pub fn cmd_sensors_show(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    let s = &ctx.sensing;
    let snapshot = s.monitor.snapshot();
    out!(
        "Sources:      internal ADC {}, external ADC {}, inputs {}, power {}\n",
        SourceName(s.analog.internal_source()),
        SourceName(s.analog.external_source()),
        SourceName(s.io.fault_source()),
        SourceName(s.io.power_source())
    );
    if let Some(fault) = s.adc_fault {
        out!("ADC fault:    {}\n", fault.name());
    }
    out!(
        "Sampling:     every {} ms, trace {}\n",
        ipmc_defs::IPMC_SENSOR_PERIOD_MS,
        s.monitor.trace_level().name()
    );
    out!("Rails:\n");
    for (index, (reading, def)) in snapshot.valid_rails().iter().zip(RAILS.iter()).enumerate() {
        out!(
            "  {} {:<12} {:>6} mV  {}  ({})\n",
            index,
            def.name,
            reading.mv,
            reading.status.name(),
            SourceName(rail_source(s, index))
        );
    }
    let t = &snapshot.temperature;
    if t.available {
        out!(
            "Die temp:     {} C  flags {:02X}h  ({})\n",
            Tenths(t.dc),
            t.flags,
            SourceName(s.analog.internal_source())
        );
    } else {
        out!("Die temp:     unavailable\n");
    }
    out!("Inputs:\n");
    for (index, def) in INPUTS.iter().enumerate() {
        let bit = 1u32 << index;
        let state = if snapshot.inputs_unreadable & bit != 0 {
            "unreadable"
        } else if snapshot.inputs_asserted & bit != 0 {
            "asserted"
        } else {
            "clear"
        };
        out!("  {:<16} {}\n", def.name, state);
    }
    out!(
        "Power:        {} (PWR_EN {})\n",
        snapshot.power.name(),
        on_off_text(snapshot.power_enabled)
    );
    let dispatch = ctx.ipmb.dispatch();
    let fru_state = &dispatch.fru_state;
    out!(
        "FRU state:    {} (in this state since {} ms)\n",
        fru_state.state().name(),
        fru_state.since_ms()
    );
    out!("Tier-1 sensors:\n");
    let buses = &ctx.ipmb.link().view().buses;
    let temperature_text = if t.available {
        "available"
    } else {
        "unavailable"
    };
    let rows: [(&str, u8, &str); 7] = [
        ("FRU State", SENSOR_FRU_STATE, fru_state.state().name()),
        (
            "IPMB Link",
            SENSOR_SYSTEM_IPMB_LINK,
            "from the link layer's bus states",
        ),
        ("FRU Health", SENSOR_FRU_HEALTH, snapshot.health.name()),
        ("FRU Voltage", SENSOR_FRU_VOLTAGE, snapshot.voltage.name()),
        ("FRU Temperature", SENSOR_FRU_TEMPERATURE, temperature_text),
        (
            "Payload Test Results",
            SENSOR_PAYLOAD_TEST_RESULTS,
            "unavailable, no payload test has run",
        ),
        (
            "Payload Test Status",
            SENSOR_PAYLOAD_TEST_STATUS,
            "not in progress",
        ),
    ];
    for (label, number, summary) in rows {
        print_reading(dispatch, buses, label, number, summary);
    }
    if dispatch.analog.count() > 0 {
        out!(
            "Analog sensors (Full SDR records; value x M x 10^R):
"
        );
        for (index, spec) in SPECS.iter().enumerate().take(dispatch.analog.count()) {
            let number = ipmc_core::analog::ANALOG_FIRST + index as u8;
            print_reading(dispatch, buses, spec.name, number, "from the sample above");
        }
    }
    Ok(())
}

/// cmd_sensors_trace -- `SENSORS TRACE {OFF|EVENTS|ALL}`: sets, or with no argument reports,
/// the sensor trace level. Sampling runs at every level; only the printing changes.
pub fn cmd_sensors_trace(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    if let Some(word) = optional(args.text_at(0))? {
        let Some(level) = Level::from_name(word) else {
            out!(
                "Error: '{}' is not a trace level (OFF, EVENTS or ALL)\n",
                word
            );
            return Err(CmdError::Reported);
        };
        ctx.sensing.monitor.set_trace_level(level);
    }
    out!(
        "Sensor trace: {}\n",
        ctx.sensing.monitor.trace_level().name()
    );
    Ok(())
}

// ---- POWER ----

/// report_power -- one status line for the power controller.
fn report_power(sensing: &Sensing) {
    let power = sensing.monitor.power();
    out!(
        "Power: {}, PWR_EN {}",
        power.state().name(),
        on_off_text(sensing.io.is_enabled())
    );
    match power.fault() {
        Some(fault) => out!(", latched fault: {}\n", fault),
        None => out!(", no fault latched\n"),
    }
}

/// power_result -- prints the outcome of a power command and resamples so the snapshot follows.
fn power_result(sensing: &mut Sensing, result: Result<(), PowerError>) -> CmdResult {
    match result {
        Ok(()) => {
            sensing.sample_now();
            report_power(sensing);
            Ok(())
        }
        Err(e) => {
            if matches!(e, PowerError::OutputWrite(_)) {
                // The failed PWR_EN write latched a fault; resample so the snapshot follows.
                sensing.sample_now();
            }
            out!("Error: {}\n", e);
            Err(CmdError::Reported)
        }
    }
}

/// run_power -- runs one power-controller operation on the monitor and the power router, then
/// prints its outcome (`power_result`).
fn run_power(
    sensing: &mut Sensing,
    op: impl FnOnce(&mut Monitor, &mut RoutedIo<BoardIo>, &mut ConsoleTrace) -> Result<(), PowerError>,
) -> CmdResult {
    let result = op(&mut sensing.monitor, &mut sensing.io, &mut ConsoleTrace);
    power_result(sensing, result)
}

/// cmd_power_on -- asserts PWR_EN and waits for PGOOD (timeout IPMC_PGOOD_TIMEOUT_MS). Refused
/// while the FRU is in M6 or activation-locked (`IpmbLink::power_on_check`).
pub fn cmd_power_on(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    if let Err(refused) = ctx.ipmb.power_on_check() {
        out!(
            "Error: {}
",
            refused.name()
        );
        return Err(CmdError::Reported);
    }
    run_power(&mut ctx.sensing, |monitor, io, trace| {
        monitor.power_on(systick::millis(), io, trace)
    })
}

/// cmd_power_off -- releases PWR_EN and clears a latched fault.
pub fn cmd_power_off(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    run_power(&mut ctx.sensing, |monitor, io, trace| {
        monitor.power_off(io, trace)
    })
}

/// cmd_power_clear -- clears a latched power fault.
pub fn cmd_power_clear(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    run_power(&mut ctx.sensing, |monitor, io, trace| {
        monitor.power_clear(io, trace)
    })
}

/// cmd_power_status -- the power state, the enable output and the latched fault.
pub fn cmd_power_status(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    report_power(&ctx.sensing);
    out!(
        "PGOOD timeout: {} ms, power at boot: {}, enable source: {}\n",
        ipmc_defs::IPMC_PGOOD_TIMEOUT_MS,
        on_off_text(ipmc_defs::IPMC_POWER_AT_BOOT),
        SourceName(ctx.sensing.io.power_source())
    );
    Ok(())
}

// ---- SIM ----

/// print_sim_rail -- one simulated rail line.
fn print_sim_rail(sim: &SimAnalog, index: usize) {
    let Some(def) = RAILS.get(index) else { return };
    match sim.rail_value(def.channel) {
        Ok(mv) => out!(
            "  {} {:<12} {:>6} mV (nominal {})\n",
            index,
            def.name,
            mv,
            def.nominal_mv
        ),
        Err(e) => out!("  {} {:<12} {}\n", index, def.name, e.name()),
    }
}

/// cmd_sim_rail -- `SIM RAIL {N} {MV}`: no argument lists the simulated rails, N shows one, N MV
/// sets it (millivolts at the rail, after the divider).
pub fn cmd_sim_rail(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    let s = &mut ctx.sensing;
    let Some(index) = optional(args.u32_at(0))? else {
        out!("Simulated rails (rail mV, 0 = VDDA):\n");
        for index in 0..RAILS.len() {
            print_sim_rail(&s.analog.sim, index);
        }
        return Ok(());
    };
    let index = usize::try_from(index).unwrap_or(usize::MAX);
    let Some(def) = RAILS.get(index) else {
        out!("Error: rail must be 0..{}\n", RAILS.len() - 1);
        return Err(CmdError::Reported);
    };
    if rail_source(s, index) != Source::Sim {
        let define = if matches!(def.channel, Channel::Vdda) {
            "IPMC_ADC_INTERNAL_SOURCE"
        } else {
            "IPMC_ADC_EXTERNAL_SOURCE"
        };
        return not_simulated("rail", define);
    }
    if let Some(mv) = optional(args.u32_at(1))? {
        if s.analog.sim.set_rail_mv(def.channel, mv).is_err() {
            out!("Error: rail {} is not in the simulator\n", index);
            return Err(CmdError::Reported);
        }
        s.sample_now();
    }
    print_sim_rail(&s.analog.sim, index);
    Ok(())
}

/// cmd_sim_temp -- `SIM TEMP {DC}`: sets or shows the simulated die temperature in tenths of a
/// degree C.
pub fn cmd_sim_temp(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    let s = &mut ctx.sensing;
    if s.analog.internal_source() != Source::Sim {
        return not_simulated("die temperature", "IPMC_ADC_INTERNAL_SOURCE");
    }
    if let Some(dc) = optional(args.i32_at(0))? {
        s.analog.sim.set_temperature_dc(dc);
        s.sample_now();
    }
    out!(
        "Simulated die temperature: {} C\n",
        Tenths(s.analog.sim.temperature_dc())
    );
    Ok(())
}

/// print_sim_input -- one simulated input line: asserted or clear, and the pin level.
fn print_sim_input(sim: &mut SimInputs, index: usize) {
    let Some(def) = INPUTS.get(index) else { return };
    match sim.level_high(def.input) {
        Ok(high) => out!(
            "  {:<16} {} (pin {})\n",
            def.name,
            if high == def.active_high {
                "asserted"
            } else {
                "clear"
            },
            if high { "high" } else { "low" }
        ),
        Err(e) => out!("  {:<16} {}\n", def.name, e.name()),
    }
}

/// cmd_sim_fault -- `SIM FAULT {NAME} {ON|OFF}`: no argument lists the simulated inputs, NAME
/// shows one, NAME ON asserts it (the pin at its active level), NAME OFF clears it.
pub fn cmd_sim_fault(ctx: &mut Context, args: &Args<'_>) -> CmdResult {
    let s = &mut ctx.sensing;
    let Some(name) = optional(args.text_at(0))? else {
        out!("Simulated inputs (ON = asserted; PGOOD asserted means power good):\n");
        for index in 0..INPUTS.len() {
            print_sim_input(&mut s.io.sim_inputs, index);
        }
        return Ok(());
    };
    let Some(index) = faults::find_by_name(&INPUTS, name) else {
        out!("Error: unknown input '{}' (see SIM FAULT)\n", name);
        return Err(CmdError::Reported);
    };
    if s.io.fault_source() != Source::Sim {
        return not_simulated("fault inputs", "IPMC_FAULT_SOURCE");
    }
    let Some(def) = INPUTS.get(index) else {
        return Err(CmdError::Reported);
    };
    if let Some(on) = optional(args.on_off_at(1))? {
        // Asserted means the pin is at its active level.
        s.io.sim_inputs.set_level(def.input, on == def.active_high);
        s.sample_now();
    }
    print_sim_input(&mut s.io.sim_inputs, index);
    Ok(())
}

/// cmd_sim_show -- every group with its source and, for a simulated group, its values.
pub fn cmd_sim_show(ctx: &mut Context, _args: &Args<'_>) -> CmdResult {
    let s = &mut ctx.sensing;
    out!(
        "Internal ADC (VDDA, die temperature): {}\n",
        SourceName(s.analog.internal_source())
    );
    out!(
        "External ADC (rails 1..4):             {}\n",
        SourceName(s.analog.external_source())
    );
    out!(
        "Fault inputs:                          {}\n",
        SourceName(s.io.fault_source())
    );
    out!(
        "Power enable:                          {}\n",
        SourceName(s.io.power_source())
    );
    for index in 0..RAILS.len() {
        if rail_source(s, index) == Source::Sim {
            print_sim_rail(&s.analog.sim, index);
        } else if let Some(def) = RAILS.get(index) {
            out!("  {} {:<12} not simulated\n", index, def.name);
        }
    }
    if s.analog.internal_source() == Source::Sim {
        out!(
            "  Die temperature {} C\n",
            Tenths(s.analog.sim.temperature_dc())
        );
    } else {
        out!("  Die temperature not simulated\n");
    }
    if s.io.fault_source() == Source::Sim {
        for index in 0..INPUTS.len() {
            print_sim_input(&mut s.io.sim_inputs, index);
        }
    } else {
        out!("  Fault inputs not simulated\n");
    }
    let snapshot = s.monitor.snapshot();
    out!(
        "Latest sample: power {}, asserted {}, unreadable {}\n",
        snapshot.power.name(),
        Asserted(snapshot.inputs_asserted),
        Asserted(snapshot.inputs_unreadable)
    );
    Ok(())
}

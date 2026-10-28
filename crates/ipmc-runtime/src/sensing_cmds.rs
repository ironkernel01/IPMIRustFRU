// sensing_cmds.rs - the SENSORS, POWER and SIM console groups: the handlers over the sensor service
// (`crate::sensing::Sensing`), the Monitor and the routers.
//
// The Monitor service (sampling, the power state machine, the payload reset pulse) is
// `Sensing`; the decisions live in ipmc-sensors and ipmc-core; this file moves values between them
// and the console. console.rs only registers the tables.
//
// Trace lines go out through ipmc_core::trace::Trace, implemented by `ConsoleTrace` over the
// console port. Nothing here runs in an interrupt handler.

use core::fmt;

use ipmc_core::dispatch::Context as DispatchContext;
use ipmc_core::link::BusPair;
use ipmc_core::sensor::{
    READING_LEN_MAX, SENSOR_FRU_HEALTH, SENSOR_FRU_STATE, SENSOR_FRU_TEMPERATURE,
    SENSOR_FRU_VOLTAGE, SENSOR_PAYLOAD_TEST_RESULTS, SENSOR_PAYLOAD_TEST_STATUS,
    SENSOR_SYSTEM_IPMB_LINK,
};
use ipmc_hal::board::Board;
use ipmc_hal::time::Clock;
use ipmc_menu::{on_off_text, optional, Args, CmdError, CmdResult};
use ipmc_sensors::analog::SPECS;
use ipmc_sensors::board_sensors::{INPUTS, RAILS};
use ipmc_sensors::drivers::sim::{SimAnalog, SimInputs};
use ipmc_sensors::eval::faults;
use ipmc_sensors::eval::power_fsm::PowerError;
use ipmc_sensors::hal::{
    AnalogSource, Channel, DigitalInputs, PowerSwitch, RailWiring, ResetOutput,
};
use ipmc_sensors::monitor::Monitor;
use ipmc_sensors::routing::{RoutedIo, Source};
use ipmc_sensors::trace::{Asserted, Level, Tenths};

use crate::console::{Context, LinkHome};
use crate::out;
use crate::sensing::{BoardSensing, Sensing};
use crate::text::{ConsoleText, ConsoleTrace};

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
    con: &mut impl ConsoleText,
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
                con,
                "  {} (sensor {}): {}\n    Get Sensor Reading data:",
                label,
                number,
                summary
            );
            for byte in bytes.iter().take(len) {
                out!(con, " {:02X}", byte);
            }
            out!(con, "\n");
        }
        Err(e) => out!(
            con,
            "  {} (sensor {}): read failed: {:?}\n",
            label,
            number,
            e
        ),
    }
}

/// not_simulated -- prints that a group is not simulated; the command fails as reported.
fn not_simulated(con: &mut impl ConsoleText, group: &str, define: &str) -> CmdResult {
    out!(
        con,
        "Not simulated: the {} source is hardware (ipmc-defs {})\n",
        group,
        define
    );
    Err(CmdError::Reported)
}

/// rail_source -- the backend of rail `index` of the board table.
fn rail_source<A: AnalogSource, Io: DigitalInputs + PowerSwitch + ResetOutput>(
    sensing: &Sensing<A, Io>,
    index: usize,
) -> Source {
    match sensing.monitor.wiring().get(index).map(|wire| wire.channel) {
        Some(Channel::Vdda) => sensing.analog.internal_source(),
        _ => sensing.analog.external_source(),
    }
}

// ---- SENSORS ----

/// cmd_sensors_show -- every rail, the die temperature, the inputs, the power state and the
/// three Tier-1 readings with their Get Sensor Reading data bytes.
pub fn cmd_sensors_show<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    _args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    let s = &ctx.sensing;
    let snapshot = s.monitor.snapshot();
    out!(
        con,
        "Sources:      internal ADC {}, external ADC {}, inputs {}, power {}\n",
        SourceName(s.analog.internal_source()),
        SourceName(s.analog.external_source()),
        SourceName(s.io.fault_source()),
        SourceName(s.io.power_source())
    );
    if let Some(fault) = s.adc_fault {
        out!(con, "ADC fault:    {}\n", fault);
    }
    out!(
        con,
        "Sampling:     every {} ms, trace {}\n",
        ipmc_defs::IPMC_SENSOR_PERIOD_MS,
        s.monitor.trace_level().name()
    );
    out!(con, "Rails:\n");
    for (index, (reading, def)) in snapshot.valid_rails().iter().zip(RAILS.iter()).enumerate() {
        out!(
            con,
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
            con,
            "Die temp:     {} C  flags {:02X}h  ({})\n",
            Tenths(t.dc),
            t.flags,
            SourceName(s.analog.internal_source())
        );
    } else {
        out!(con, "Die temp:     unavailable\n");
    }
    out!(con, "Inputs:\n");
    for (index, def) in INPUTS.iter().enumerate() {
        let bit = 1u32 << index;
        let state = if snapshot.inputs_unreadable & bit != 0 {
            "unreadable"
        } else if snapshot.inputs_asserted & bit != 0 {
            "asserted"
        } else {
            "clear"
        };
        out!(con, "  {:<16} {}\n", def.name, state);
    }
    out!(
        con,
        "Power:        {} (PWR_EN {})\n",
        snapshot.power.name(),
        on_off_text(snapshot.power_enabled)
    );
    let dispatch = ctx.ipmb.dispatch();
    let fru_state = &dispatch.fru_state;
    out!(
        con,
        "FRU state:    {} (in this state since {} ms)\n",
        fru_state.state().name(),
        fru_state.since_ms()
    );
    out!(con, "Tier-1 sensors:\n");
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
        print_reading(con, dispatch, buses, label, number, summary);
    }
    if dispatch.analog.count() > 0 {
        out!(
            con,
            "Analog sensors (Full SDR records; value x M x 10^R):
"
        );
        for (index, spec) in SPECS.iter().enumerate().take(dispatch.analog.count()) {
            let number = ipmc_core::analog::ANALOG_FIRST + index as u8;
            print_reading(
                con,
                dispatch,
                buses,
                spec.name,
                number,
                "from the sample above",
            );
        }
    }
    Ok(())
}

/// cmd_sensors_trace -- `SENSORS TRACE {OFF|EVENTS|ALL}`: sets, or with no argument reports,
/// the sensor trace level. Sampling runs at every level; only the printing changes.
pub fn cmd_sensors_trace<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    if let Some(word) = optional(args.text_at(0))? {
        let Some(level) = Level::from_name(word) else {
            out!(
                con,
                "Error: '{}' is not a trace level (OFF, EVENTS or ALL)\n",
                word
            );
            return Err(CmdError::Reported);
        };
        ctx.sensing.monitor.set_trace_level(level);
    }
    out!(
        con,
        "Sensor trace: {}\n",
        ctx.sensing.monitor.trace_level().name()
    );
    Ok(())
}

// ---- POWER ----

/// report_power -- one status line for the power controller.
fn report_power<A: AnalogSource, Io: DigitalInputs + PowerSwitch + ResetOutput>(
    con: &mut impl ConsoleText,
    sensing: &Sensing<A, Io>,
) {
    let power = sensing.monitor.power();
    out!(
        con,
        "Power: {}, PWR_EN {}",
        power.state().name(),
        on_off_text(sensing.io.is_enabled())
    );
    match power.fault() {
        Some(fault) => out!(con, ", latched fault: {}\n", fault),
        None => out!(con, ", no fault latched\n"),
    }
}

/// power_result -- prints the outcome of a power command and resamples so the snapshot follows.
fn power_result<B: Board>(
    con: &mut B::Console,
    clock: &B::Clock,
    sensing: &mut BoardSensing<B>,
    result: Result<(), PowerError>,
) -> CmdResult {
    match result {
        Ok(()) => {
            sensing.sample_now(&mut *con, clock.millis());
            report_power(con, sensing);
            Ok(())
        }
        Err(e) => {
            if matches!(e, PowerError::OutputWrite(_)) {
                // The failed PWR_EN write latched a fault; resample so the snapshot follows.
                sensing.sample_now(&mut *con, clock.millis());
            }
            out!(con, "Error: {}\n", e);
            Err(CmdError::Reported)
        }
    }
}

/// run_power -- runs one power-controller operation on the monitor and the power router, then
/// prints its outcome (`power_result`).
fn run_power<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    op: impl FnOnce(
        &mut Monitor,
        &mut RoutedIo<B::Io>,
        &mut ConsoleTrace<'_, B::Console>,
    ) -> Result<(), PowerError>,
) -> CmdResult {
    let sensing = &mut ctx.sensing;
    let result = op(
        &mut sensing.monitor,
        &mut sensing.io,
        &mut ConsoleTrace(&mut ctx.con),
    );
    power_result::<B>(&mut ctx.con, &ctx.clock, sensing, result)
}

/// cmd_power_on -- asserts PWR_EN and waits for PGOOD (timeout IPMC_PGOOD_TIMEOUT_MS). Refused
/// while the FRU is in M6 or activation-locked (`IpmbLink::power_on_check`).
pub fn cmd_power_on<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    _args: &Args<'_>,
) -> CmdResult {
    if let Err(refused) = ctx.ipmb.power_on_check() {
        out!(
            &mut ctx.con,
            "Error: {}
",
            refused.name()
        );
        return Err(CmdError::Reported);
    }
    let clock = ctx.clock.clone();
    run_power(ctx, |monitor, io, trace| {
        monitor.power_on(clock.millis(), io, trace)
    })
}

/// cmd_power_off -- releases PWR_EN and clears a latched fault.
pub fn cmd_power_off<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    _args: &Args<'_>,
) -> CmdResult {
    run_power(ctx, |monitor, io, trace| monitor.power_off(io, trace))
}

/// cmd_power_clear -- clears a latched power fault.
pub fn cmd_power_clear<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    _args: &Args<'_>,
) -> CmdResult {
    run_power(ctx, |monitor, io, trace| monitor.power_clear(io, trace))
}

/// cmd_power_status -- the power state, the enable output and the latched fault.
pub fn cmd_power_status<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    _args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    report_power(con, &ctx.sensing);
    out!(
        con,
        "PGOOD timeout: {} ms, power at boot: {}, enable source: {}\n",
        ipmc_defs::IPMC_PGOOD_TIMEOUT_MS,
        on_off_text(ctx.start.power_at_boot),
        SourceName(ctx.sensing.io.power_source())
    );
    Ok(())
}

// ---- SIM ----

/// print_sim_rail -- one simulated rail line.
fn print_sim_rail(
    con: &mut impl ConsoleText,
    sim: &SimAnalog,
    wiring: &[RailWiring],
    index: usize,
) {
    let (Some(def), Some(wire)) = (RAILS.get(index), wiring.get(index)) else {
        return;
    };
    match sim.rail_value(wire.channel) {
        Ok(mv) => out!(
            con,
            "  {} {:<12} {:>6} mV (nominal {})\n",
            index,
            def.name,
            mv,
            def.nominal_mv
        ),
        Err(e) => out!(con, "  {} {:<12} {}\n", index, def.name, e.name()),
    }
}

/// cmd_sim_rail -- `SIM RAIL {N} {MV}`: no argument lists the simulated rails, N shows one, N MV
/// sets it (millivolts at the rail, after the divider).
pub fn cmd_sim_rail<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    let s = &mut ctx.sensing;
    let Some(index) = optional(args.u32_at(0))? else {
        out!(con, "Simulated rails (rail mV, 0 = VDDA):\n");
        for index in 0..RAILS.len() {
            print_sim_rail(con, &s.analog.sim, s.monitor.wiring(), index);
        }
        return Ok(());
    };
    let index = usize::try_from(index).unwrap_or(usize::MAX);
    let Some(wire) = s.monitor.wiring().get(index) else {
        out!(con, "Error: rail must be 0..{}\n", RAILS.len() - 1);
        return Err(CmdError::Reported);
    };
    let channel = wire.channel;
    if rail_source(s, index) != Source::Sim {
        let define = if matches!(channel, Channel::Vdda) {
            "IPMC_ADC_INTERNAL_SOURCE"
        } else {
            "IPMC_ADC_EXTERNAL_SOURCE"
        };
        return not_simulated(con, "rail", define);
    }
    if let Some(mv) = optional(args.u32_at(1))? {
        if s.analog.sim.set_rail_mv(channel, mv).is_err() {
            out!(con, "Error: rail {} is not in the simulator\n", index);
            return Err(CmdError::Reported);
        }
        s.sample_now(&mut *con, ctx.clock.millis());
    }
    print_sim_rail(con, &s.analog.sim, s.monitor.wiring(), index);
    Ok(())
}

/// cmd_sim_temp -- `SIM TEMP {DC}`: sets or shows the simulated die temperature in tenths of a
/// degree C.
pub fn cmd_sim_temp<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    let s = &mut ctx.sensing;
    if s.analog.internal_source() != Source::Sim {
        return not_simulated(con, "die temperature", "IPMC_ADC_INTERNAL_SOURCE");
    }
    if let Some(dc) = optional(args.i32_at(0))? {
        s.analog.sim.set_temperature_dc(dc);
        s.sample_now(&mut *con, ctx.clock.millis());
    }
    out!(
        con,
        "Simulated die temperature: {} C\n",
        Tenths(s.analog.sim.temperature_dc())
    );
    Ok(())
}

/// print_sim_input -- one simulated input line: asserted or clear, and the pin level.
fn print_sim_input(con: &mut impl ConsoleText, sim: &mut SimInputs, index: usize) {
    let Some(def) = INPUTS.get(index) else { return };
    let high = match sim.level_high(def.input) {
        Ok(high) => high,
        Err(e) => {
            out!(con, "  {:<16} {}\n", def.name, e.name());
            return;
        }
    };
    let state = if high == def.active_high {
        "asserted"
    } else {
        "clear"
    };
    out!(
        con,
        "  {:<16} {} (pin {})\n",
        def.name,
        state,
        if high { "high" } else { "low" }
    );
}

/// cmd_sim_fault -- `SIM FAULT {NAME} {ON|OFF}`: no argument lists the simulated inputs, NAME
/// shows one, NAME ON asserts it (the pin at its active level), NAME OFF clears it.
pub fn cmd_sim_fault<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    let s = &mut ctx.sensing;
    let Some(name) = optional(args.text_at(0))? else {
        out!(
            con,
            "Simulated inputs (ON = asserted; PGOOD asserted means power good):\n"
        );
        for index in 0..INPUTS.len() {
            print_sim_input(con, &mut s.io.sim_inputs, index);
        }
        return Ok(());
    };
    let Some(index) = faults::find_by_name(&INPUTS, name) else {
        out!(con, "Error: unknown input '{}' (see SIM FAULT)\n", name);
        return Err(CmdError::Reported);
    };
    if s.io.fault_source() != Source::Sim {
        return not_simulated(con, "fault inputs", "IPMC_FAULT_SOURCE");
    }
    let Some(def) = INPUTS.get(index) else {
        return Err(CmdError::Reported);
    };
    if let Some(on) = optional(args.on_off_at(1))? {
        // Asserted means the pin is at its active level.
        s.io.sim_inputs.set_level(def.input, on == def.active_high);
        s.sample_now(&mut *con, ctx.clock.millis());
    }
    print_sim_input(con, &mut s.io.sim_inputs, index);
    Ok(())
}

/// cmd_sim_show -- every group with its source and, for a simulated group, its values.
pub fn cmd_sim_show<B: Board, L: LinkHome<B>>(
    ctx: &mut Context<B, L>,
    _args: &Args<'_>,
) -> CmdResult {
    let con = &mut ctx.con;
    let s = &mut ctx.sensing;
    out!(
        con,
        "Internal ADC (VDDA, die temperature): {}\n",
        SourceName(s.analog.internal_source())
    );
    out!(
        con,
        "External ADC (rails 1..4):             {}\n",
        SourceName(s.analog.external_source())
    );
    out!(
        con,
        "Fault inputs:                          {}\n",
        SourceName(s.io.fault_source())
    );
    out!(
        con,
        "Power enable:                          {}\n",
        SourceName(s.io.power_source())
    );
    for index in 0..RAILS.len() {
        if rail_source(s, index) == Source::Sim {
            print_sim_rail(con, &s.analog.sim, s.monitor.wiring(), index);
        } else if let Some(def) = RAILS.get(index) {
            out!(con, "  {} {:<12} not simulated\n", index, def.name);
        }
    }
    if s.analog.internal_source() == Source::Sim {
        out!(
            con,
            "  Die temperature {} C\n",
            Tenths(s.analog.sim.temperature_dc())
        );
    } else {
        out!(con, "  Die temperature not simulated\n");
    }
    if s.io.fault_source() == Source::Sim {
        for index in 0..INPUTS.len() {
            print_sim_input(con, &mut s.io.sim_inputs, index);
        }
    } else {
        out!(con, "  Fault inputs not simulated\n");
    }
    let snapshot = s.monitor.snapshot();
    out!(
        con,
        "Latest sample: power {}, asserted {}, unreadable {}\n",
        snapshot.power.name(),
        Asserted(snapshot.inputs_asserted),
        Asserted(snapshot.inputs_unreadable)
    );
    Ok(())
}

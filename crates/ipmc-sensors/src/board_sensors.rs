//! board_sensors.rs - the one file that says what this board measures and how each Tier-1
//! sensor reading is built. Read this file first.
//!
//! Drivers: each group is backed by the driver its ipmi_defs.h switch names (routing.rs): the
//! on-chip ADC and GPIO (drivers/stm32f401), the simulators (drivers/sim), or an external part
//! (drivers/i2c, drivers/spi; no bus assigned yet). Adding a driver: see drivers/*/mod.rs.
//!
//! Values: every number and every name below comes from a define in ipmi_defs.h (read here as
//! `ipmc_defs::NAME`); this file holds the tables and the logic that combines them.
//!
//! HOW EACH VALUE IS READ
//!
//! | Value | Source | How it is read |
//! |---|---|---|
//! | VDDA (rail 0) | internal (VREFINT) | VDDA = 3300 mV x VREFIN_CAL / reading (temperature.rs) |
//! | Die temperature | internal | reading rescaled to 3300 mV, then the line through TS_CAL1 (30 C) and TS_CAL2 (110 C), in tenths of a degree C |
//! | 12V payload (rail 1) | external input 0 | pin mV = reading x VDDA / 4095, rail mV = pin mV x 4 / 1 |
//! | 5V (rail 2) | external input 1 | pin mV x 2 / 1 |
//! | 3V3 payload (rail 3) | external input 2 | pin mV x 2 / 1 |
//! | +3.3V_AUX (rail 4) | external input 3 | pin mV x 2 / 1 |
//! | PGOOD | digital input | asserted when high; unreadable counts as not good |
//! | FAULT_N | digital input | asserted when low (a jumper to GND); unreadable makes health bad |
//! | OVERTEMP_N | digital input | asserted when low; unreadable makes health bad |
//! | PAYLOAD_RESET_N | digital input | asserted when low; shown, not part of health |
//! | PWR_EN | digital output | driven by the power state machine (eval/power_fsm.rs) |
//!
//! Pins and ADC channels are not named here: the board support chooses them once, in
//! ipmc-nucleo-f401re `bsp/board_pins.rs` (external input n is the n-th entry of its ADC table). Which pin
//! carries which signal is also listed in Documentation/nucleo_f401re_pinout.md.
//!
//! Each group has a source switch in ipmi_defs.h (IPMC_ADC_INTERNAL_SOURCE,
//! IPMC_ADC_EXTERNAL_SOURCE, IPMC_FAULT_SOURCE, IPMC_POWER_SOURCE): hardware or simulation. On the
//! bench the A0..A3 pins are unwired, so the external rails are simulated and start at nominal.
//!
//! HOW EACH TIER-1 READING IS BUILT (`build_snapshot`, then ipmc_core::sensor encoders)
//!
//! - Sensor 3, FRU Voltage (type 02h): good when every rail of `RAILS` is in its non-critical
//!   window (+/- IPMC_RAIL_NONCRIT_PCT of nominal); bad when any rail is outside it. A rail
//!   that could not be read makes the reading unavailable unless another rail is already bad.
//! - Sensor 4, FRU Temperature (type F3h): the die temperature against `TEMPERATURE_LIMITS`
//!   (LNC 0, LC -10, UNC 70, UC 85, UNR 100 C). The state byte carries one bit per threshold
//!   reached: at or below for the lower two, at or above for the upper three. There is no lower
//!   non-recoverable threshold, so bit 2 stays 0. A Tier-1 IPMC sends no events; the bits only
//!   show in the polled reading.
//! - Sensor 2, FRU Health (type F2h): bad when FAULT_N or OVERTEMP_N is asserted, or PWR_EN is
//!   on while PGOOD is low, or any rail is critical (beyond +/- IPMC_RAIL_CRIT_PCT), or the die
//!   temperature is at or above the upper critical threshold; otherwise good.
//!
//! Evaluation lives elsewhere and exists once: rail windows in rail.rs, temperature in
//! temperature.rs, inputs in faults.rs, power sequencing in power_fsm.rs, byte encoding in
//! ipmc-core sensor.rs.

use crate::drivers::sim::SimAnalog;
use crate::eval::faults::{FaultEval, InputDef};
use crate::eval::rail::{self, RailDef};
use crate::eval::temperature::{self, Thresholds};
use crate::hal::{AnalogSource, Channel, Divider, Input};
use ipmc_core::sensor::{
    PowerState, RailReading, RailStatus, SensorSnapshot, Summary, TemperatureReading, TEMP_FLAG_UC,
};

/// RAIL_COUNT -- number of rails in the board table (ipmc-defs guards IPMC_RAILS_MAX against it).
pub const RAIL_COUNT: usize = 5;

/// TENTHS -- tenths of a degree per degree (the unit of every temperature in this crate).
pub(crate) const TENTHS: i32 = 10;

/// dc_from_define -- an ipmi_defs.h temperature (signed degrees C) in tenths of a degree C.
/// The defines are guarded to -40..=125 C, so the product cannot overflow.
const fn dc_from_define(degrees_c: i32) -> i32 {
    degrees_c * TENTHS
}

/// EXTERNAL_INPUTS -- number of external analog inputs (rails 1..): the rails behind a divider.
pub const EXTERNAL_INPUTS: usize = RAIL_COUNT - 1;

/// Abstract channel ids of the external rails: external input n (A0 = 0 .. A3 = 3). The pin and
/// ADC channel behind each id are chosen once, by the board support (ipmc-nucleo-f401re bsp/board_pins.rs).
const INPUT_A0: u8 = 0;
const INPUT_A1: u8 = 1;
const INPUT_A2: u8 = 2;
const INPUT_A3: u8 = 3;

/// RAILS -- the monitored rails, in table order; the SIM console numbers rails by this index.
pub const RAILS: [RailDef; RAIL_COUNT] = [
    RailDef {
        name: ipmc_defs::IPMC_RAIL0_NAME,
        channel: Channel::Vdda,
        nominal_mv: ipmc_defs::IPMC_RAIL0_NOMINAL_MV,
        divider: Divider { num: 1, den: 1 },
        noncrit_pct: ipmc_defs::IPMC_RAIL_NONCRIT_PCT,
        crit_pct: ipmc_defs::IPMC_RAIL_CRIT_PCT,
    },
    RailDef {
        name: ipmc_defs::IPMC_RAIL1_NAME,
        channel: Channel::Adc(INPUT_A0),
        nominal_mv: ipmc_defs::IPMC_RAIL1_NOMINAL_MV,
        divider: Divider {
            num: ipmc_defs::IPMC_RAIL1_DIV_NUM,
            den: ipmc_defs::IPMC_RAIL1_DIV_DEN,
        },
        noncrit_pct: ipmc_defs::IPMC_RAIL_NONCRIT_PCT,
        crit_pct: ipmc_defs::IPMC_RAIL_CRIT_PCT,
    },
    RailDef {
        name: ipmc_defs::IPMC_RAIL2_NAME,
        channel: Channel::Adc(INPUT_A1),
        nominal_mv: ipmc_defs::IPMC_RAIL2_NOMINAL_MV,
        divider: Divider {
            num: ipmc_defs::IPMC_RAIL2_DIV_NUM,
            den: ipmc_defs::IPMC_RAIL2_DIV_DEN,
        },
        noncrit_pct: ipmc_defs::IPMC_RAIL_NONCRIT_PCT,
        crit_pct: ipmc_defs::IPMC_RAIL_CRIT_PCT,
    },
    RailDef {
        name: ipmc_defs::IPMC_RAIL3_NAME,
        channel: Channel::Adc(INPUT_A2),
        nominal_mv: ipmc_defs::IPMC_RAIL3_NOMINAL_MV,
        divider: Divider {
            num: ipmc_defs::IPMC_RAIL3_DIV_NUM,
            den: ipmc_defs::IPMC_RAIL3_DIV_DEN,
        },
        noncrit_pct: ipmc_defs::IPMC_RAIL_NONCRIT_PCT,
        crit_pct: ipmc_defs::IPMC_RAIL_CRIT_PCT,
    },
    RailDef {
        name: ipmc_defs::IPMC_RAIL4_NAME,
        channel: Channel::Adc(INPUT_A3),
        nominal_mv: ipmc_defs::IPMC_RAIL4_NOMINAL_MV,
        divider: Divider {
            num: ipmc_defs::IPMC_RAIL4_DIV_NUM,
            den: ipmc_defs::IPMC_RAIL4_DIV_DEN,
        },
        noncrit_pct: ipmc_defs::IPMC_RAIL_NONCRIT_PCT,
        crit_pct: ipmc_defs::IPMC_RAIL_CRIT_PCT,
    },
];

const _: () = assert!(
    RAIL_COUNT <= ipmc_core::sensor::RAILS_MAX,
    "the board rail table does not fit the snapshot (IPMC_RAILS_MAX)"
);

/// TEMPERATURE_LIMITS -- LNC 0, LC -10, UNC 70, UC 85, UNR 100 C, in tenths of a degree.
pub const TEMPERATURE_LIMITS: Thresholds = Thresholds {
    lower_noncritical_dc: dc_from_define(ipmc_defs::IPMC_TEMP_LNC_C),
    lower_critical_dc: dc_from_define(ipmc_defs::IPMC_TEMP_LC_C),
    upper_noncritical_dc: dc_from_define(ipmc_defs::IPMC_TEMP_UNC_C),
    upper_critical_dc: dc_from_define(ipmc_defs::IPMC_TEMP_UC_C),
    upper_nonrecoverable_dc: dc_from_define(ipmc_defs::IPMC_TEMP_UNR_C),
};

/// SIM_TEMPERATURE_DC -- the simulated die temperature at start-up, tenths of a degree C.
pub const SIM_TEMPERATURE_DC: i32 = dc_from_define(ipmc_defs::IPMC_SIM_TEMP_C);

/// INPUTS -- the digital inputs, in bitmap order (bit n of `inputs_asserted` is entry n).
pub const INPUTS: [InputDef; Input::COUNT] = [
    InputDef {
        name: ipmc_defs::IPMC_INPUT_PGOOD_NAME,
        input: Input::Pgood,
        active_high: true,
        makes_health_bad: false, // enters health as "PWR_EN without PGOOD", below
    },
    InputDef {
        name: ipmc_defs::IPMC_INPUT_FAULT_NAME,
        input: Input::FaultN,
        active_high: false,
        makes_health_bad: true,
    },
    InputDef {
        name: ipmc_defs::IPMC_INPUT_OVERTEMP_NAME,
        input: Input::OvertempN,
        active_high: false,
        makes_health_bad: true,
    },
    InputDef {
        name: ipmc_defs::IPMC_INPUT_RESET_NAME,
        input: Input::PayloadResetN,
        active_high: false,
        makes_health_bad: false,
    },
];

/// sim_analog -- the analog simulator for this board: every rail at its nominal millivolts, the
/// die temperature at SIM_TEMPERATURE_DC (the start values of the SIM console group).
pub fn sim_analog() -> SimAnalog {
    let mut initial = [(Channel::Vdda, 0); RAIL_COUNT];
    for (slot, def) in initial.iter_mut().zip(RAILS.iter()) {
        *slot = (def.channel, def.nominal_mv);
    }
    SimAnalog::new(&initial, SIM_TEMPERATURE_DC)
}

/// read_rail -- one rail: its millivolts and window status; unavailable when unreadable.
fn read_rail<A: AnalogSource + ?Sized>(analog: &mut A, def: &RailDef) -> RailReading {
    let mv = match def.channel {
        Channel::Vdda => analog.vdda_mv(),
        Channel::Adc(channel) => analog.rail_mv(channel, def.divider),
    };
    match mv {
        Ok(mv) => RailReading {
            mv,
            status: rail::evaluate(def, mv),
        },
        Err(_) => RailReading {
            mv: 0,
            status: RailStatus::Unavailable,
        },
    }
}

/// voltage_summary -- FRU Voltage: bad if any rail is outside its non-critical window,
/// else unavailable if any rail is unreadable, else good.
fn voltage_summary(rails: &[RailReading]) -> Summary {
    let mut summary = Summary::Good;
    for reading in rails {
        match reading.status {
            RailStatus::NonCritical | RailStatus::Critical => return Summary::Bad,
            RailStatus::Unavailable => summary = Summary::Unavailable,
            RailStatus::InRange => {}
        }
    }
    summary
}

/// build_snapshot -- one sample of everything: reads the rails and the die temperature from
/// `analog`, takes the already evaluated inputs in `faults`, and derives the three Tier-1
/// summaries. `power` and `power_enabled` come from the power state machine and the switch;
/// `enable_released` says PWR_EN is known to be released (the controller requests no enable, no
/// `EnableWrite` fault is latched and the confirmed output level is off). With the PGOOD input
/// read and deasserted it sets `payload_shut_down`; the input table always holds a PGOOD input,
/// and an unreadable one leaves `payload_shut_down` false.
pub fn build_snapshot<A: AnalogSource + ?Sized>(
    analog: &mut A,
    faults: &FaultEval,
    power: PowerState,
    power_enabled: bool,
    enable_released: bool,
) -> SensorSnapshot {
    let mut snapshot = SensorSnapshot::EMPTY;
    for (slot, def) in snapshot.rails.iter_mut().zip(RAILS.iter()) {
        *slot = read_rail(analog, def);
        snapshot.rail_count += 1;
    }
    snapshot.temperature = match analog.die_temperature_dc() {
        Ok(dc) => TemperatureReading {
            available: true,
            dc,
            flags: temperature::evaluate_flags(dc, &TEMPERATURE_LIMITS),
        },
        Err(_) => TemperatureReading {
            available: false,
            dc: 0,
            flags: 0,
        },
    };
    snapshot.inputs_asserted = faults.asserted;
    snapshot.inputs_unreadable = faults.unreadable;
    snapshot.power = power;
    snapshot.power_enabled = power_enabled;
    snapshot.payload_shut_down =
        enable_released && faults.input_state(&INPUTS, Input::Pgood) == Some(false);

    snapshot.voltage = voltage_summary(snapshot.valid_rails());
    let rail_critical = snapshot
        .valid_rails()
        .iter()
        .any(|r| r.status == RailStatus::Critical);
    let temperature_critical = snapshot.temperature.flags & TEMP_FLAG_UC != 0;
    // An unreadable PGOOD counts as not good.
    let power_without_pgood =
        power_enabled && faults.input_state(&INPUTS, Input::Pgood) != Some(true);
    snapshot.health =
        if faults.health_bad || power_without_pgood || rail_critical || temperature_critical {
            Summary::Bad
        } else {
            Summary::Good
        };
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::sim::SimInputs;
    use crate::eval::faults;
    use ipmc_core::sensor::{
        encode_sensor_reading, SENSOR_FRU_HEALTH, SENSOR_FRU_TEMPERATURE, SENSOR_FRU_VOLTAGE,
        TEMP_FLAG_LNC, TEMP_FLAG_UNC, TEMP_FLAG_UNR,
    };

    const RAIL_5V: usize = 2;

    struct Bench {
        analog: SimAnalog,
        inputs: SimInputs,
    }

    impl Bench {
        fn new() -> Self {
            Bench {
                analog: sim_analog(),
                inputs: SimInputs::new(),
            }
        }

        fn snapshot(&mut self, power: PowerState, enabled: bool) -> SensorSnapshot {
            let eval = faults::evaluate(&INPUTS, &mut self.inputs);
            build_snapshot(&mut self.analog, &eval, power, enabled, !enabled)
        }
    }

    fn bytes(sensor: u8, s: &SensorSnapshot) -> [u8; 3] {
        let mut out = [0u8; 3];
        assert_eq!(encode_sensor_reading(sensor, s, &mut out), Ok(3));
        out
    }

    #[test]
    fn the_table_matches_the_documented_board() {
        let names: Vec<&str> = RAILS.iter().map(|r| r.name).collect();
        assert_eq!(
            names,
            ["VDDA", "12V payload", "5V", "3V3 payload", "+3.3V_AUX"]
        );
        let nominal: Vec<u32> = RAILS.iter().map(|r| r.nominal_mv).collect();
        assert_eq!(nominal, [3300, 12000, 5000, 3300, 3300]);
        assert_eq!(TEMPERATURE_LIMITS.lower_noncritical_dc, 0);
        assert_eq!(TEMPERATURE_LIMITS.lower_critical_dc, -100);
        assert_eq!(TEMPERATURE_LIMITS.upper_noncritical_dc, 700);
        assert_eq!(TEMPERATURE_LIMITS.upper_critical_dc, 850);
        assert_eq!(TEMPERATURE_LIMITS.upper_nonrecoverable_dc, 1000);
        assert_eq!(SIM_TEMPERATURE_DC, 250);
    }

    #[test]
    fn board_simulator_starts_at_nominal_values() {
        let mut sim = sim_analog();
        for def in RAILS.iter() {
            let value = match def.channel {
                Channel::Vdda => sim.vdda_mv(),
                Channel::Adc(ch) => sim.rail_mv(ch, def.divider),
            };
            assert_eq!(value, Ok(def.nominal_mv), "{}", def.name);
        }
        assert_eq!(sim.die_temperature_dc(), Ok(SIM_TEMPERATURE_DC));
    }

    #[test]
    fn nominal_board_is_healthy_in_range_and_cool() {
        let mut bench = Bench::new();
        let s = bench.snapshot(PowerState::On, true);
        assert_eq!(s.rail_count, RAIL_COUNT);
        assert!(s
            .valid_rails()
            .iter()
            .all(|r| r.status == RailStatus::InRange));
        assert_eq!(bytes(SENSOR_FRU_HEALTH, &s), [0x00, 0x40, 0x01]);
        assert_eq!(bytes(SENSOR_FRU_VOLTAGE, &s), [0x00, 0x40, 0x01]);
        assert_eq!(bytes(SENSOR_FRU_TEMPERATURE, &s), [0x00, 0x40, 0x00]);
    }

    #[test]
    fn critical_rail_makes_voltage_and_health_bad() {
        let mut bench = Bench::new();
        bench
            .analog
            .set_rail_mv(RAILS[RAIL_5V].channel, 4400)
            .unwrap();
        let s = bench.snapshot(PowerState::On, true);
        assert_eq!(s.rails[RAIL_5V].status, RailStatus::Critical);
        assert_eq!(bytes(SENSOR_FRU_VOLTAGE, &s), [0x00, 0x40, 0x02]);
        assert_eq!(bytes(SENSOR_FRU_HEALTH, &s), [0x00, 0x40, 0x02]);
    }

    #[test]
    fn non_critical_rail_makes_voltage_bad_but_not_health() {
        let mut bench = Bench::new();
        bench
            .analog
            .set_rail_mv(RAILS[RAIL_5V].channel, 5300)
            .unwrap();
        let s = bench.snapshot(PowerState::On, true);
        assert_eq!(s.rails[RAIL_5V].status, RailStatus::NonCritical);
        assert_eq!(s.voltage, Summary::Bad);
        assert_eq!(s.health, Summary::Good);
    }

    #[test]
    fn temperature_thresholds_reach_the_reading_and_health() {
        let mut bench = Bench::new();
        bench.analog.set_temperature_dc(750); // 75.0 C: past UNC, below UC
        let s = bench.snapshot(PowerState::On, true);
        assert_eq!(s.temperature.flags, TEMP_FLAG_UNC);
        assert_eq!(bytes(SENSOR_FRU_TEMPERATURE, &s), [0x00, 0x40, 0x08]);
        assert_eq!(s.health, Summary::Good);

        bench.analog.set_temperature_dc(850); // 85.0 C: UC reached
        let s = bench.snapshot(PowerState::On, true);
        assert_eq!(bytes(SENSOR_FRU_TEMPERATURE, &s), [0x00, 0x40, 0x18]);
        assert_eq!(s.health, Summary::Bad);

        bench.analog.set_temperature_dc(1000);
        let s = bench.snapshot(PowerState::On, true);
        assert_eq!(s.temperature.flags & TEMP_FLAG_UNR, TEMP_FLAG_UNR);

        bench.analog.set_temperature_dc(-5); // -0.5 C
        let s = bench.snapshot(PowerState::On, true);
        assert_eq!(s.temperature.flags, TEMP_FLAG_LNC);
        assert_eq!(s.health, Summary::Good);
    }

    #[test]
    fn fault_inputs_make_health_bad() {
        for input in [Input::FaultN, Input::OvertempN] {
            let mut bench = Bench::new();
            bench.inputs.set_level(input, false);
            let s = bench.snapshot(PowerState::On, true);
            assert_eq!(s.health, Summary::Bad, "{input:?}");
            assert_eq!(s.voltage, Summary::Good);
        }
        let mut bench = Bench::new();
        bench.inputs.set_level(Input::PayloadResetN, false);
        assert_eq!(bench.snapshot(PowerState::On, true).health, Summary::Good);
    }

    #[test]
    fn enable_without_pgood_is_bad_only_while_enabled() {
        let mut bench = Bench::new();
        bench.inputs.set_level(Input::Pgood, false);
        assert_eq!(
            bench.snapshot(PowerState::PowerOnRequested, true).health,
            Summary::Bad
        );
        assert_eq!(bench.snapshot(PowerState::Off, false).health, Summary::Good);
    }

    /// Inputs where one pin cannot be read.
    fn dead_pin(input: Input) -> crate::hal::digital::DeadInput {
        crate::hal::digital::DeadInput {
            dead: input,
            error: crate::hal::SourceError::BusError,
        }
    }

    #[test]
    fn unreadable_fault_input_is_kept_in_the_snapshot_and_makes_health_bad() {
        for (index, input) in [(1, Input::FaultN), (2, Input::OvertempN)] {
            let mut analog = sim_analog();
            let eval = faults::evaluate(&INPUTS, &mut dead_pin(input));
            let s = build_snapshot(&mut analog, &eval, PowerState::Off, false, true);
            assert_eq!(s.inputs_unreadable, 1 << index, "{input:?}");
            assert_eq!(s.inputs_asserted & (1 << index), 0, "{input:?}");
            assert_eq!(s.health, Summary::Bad, "{input:?}");
        }
        // PAYLOAD_RESET_N does not enter health, but its unreadability is still recorded.
        let mut analog = sim_analog();
        let eval = faults::evaluate(&INPUTS, &mut dead_pin(Input::PayloadResetN));
        let s = build_snapshot(&mut analog, &eval, PowerState::Off, false, true);
        assert_eq!(s.inputs_unreadable, 0b1000);
        assert_eq!(s.health, Summary::Good);
    }

    #[test]
    fn payload_shut_down_needs_pgood_read_low_and_pwr_en_released() {
        let mut bench = Bench::new();
        bench.inputs.set_level(Input::Pgood, false);
        assert!(bench.snapshot(PowerState::Off, false).payload_shut_down);
        let requested = bench.snapshot(PowerState::PowerOnRequested, true);
        assert!(!requested.payload_shut_down);
        bench.inputs.set_level(Input::Pgood, true);
        assert!(!bench.snapshot(PowerState::Off, false).payload_shut_down);
    }

    #[test]
    fn unreadable_pgood_counts_as_not_good_while_enabled() {
        let mut analog = sim_analog();
        let eval = faults::evaluate(&INPUTS, &mut dead_pin(Input::Pgood));
        let on = build_snapshot(
            &mut analog,
            &eval,
            PowerState::PowerOnRequested,
            true,
            false,
        );
        assert_eq!(on.health, Summary::Bad);
        let off = build_snapshot(&mut analog, &eval, PowerState::Off, false, true);
        assert_eq!(off.health, Summary::Good);
        // An unreadable PGOOD is no evidence of a shutdown, whatever PWR_EN does.
        assert!(!off.payload_shut_down);
        assert!(!on.payload_shut_down);
    }

    #[test]
    fn unreadable_sources_become_unavailable_not_invented() {
        struct Dead;
        impl AnalogSource for Dead {
            fn vdda_mv(&mut self) -> Result<u32, crate::hal::SourceError> {
                Err(crate::hal::SourceError::Unavailable)
            }
            fn die_temperature_dc(&mut self) -> Result<i32, crate::hal::SourceError> {
                Err(crate::hal::SourceError::Unavailable)
            }
            fn rail_mv(&mut self, _c: u8, _d: Divider) -> Result<u32, crate::hal::SourceError> {
                Err(crate::hal::SourceError::Unavailable)
            }
        }
        let mut inputs = SimInputs::new();
        let eval = faults::evaluate(&INPUTS, &mut inputs);
        let s = build_snapshot(&mut Dead, &eval, PowerState::Off, false, true);
        assert!(!s.temperature.available);
        assert_eq!(s.voltage, Summary::Unavailable);
        assert_eq!(bytes(SENSOR_FRU_VOLTAGE, &s), [0x00, 0x60, 0x00]);
        assert_eq!(bytes(SENSOR_FRU_TEMPERATURE, &s), [0x00, 0x60, 0x00]);
    }
}

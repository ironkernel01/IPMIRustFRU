// drivers/board_pins.rs - the board's pin and ADC channel assignments for the sensors: the one place
// that says which MCU pin carries which sensor signal. The drivers of this crate take these
// tables at construction and hold no board pin of their own. `RAIL_WIRING` says which channel
// and divider serve each rail of the sensor table, naming the external rails by abstract input
// number (entry n of `ADC.external`). The same assignment is listed for people in Documentation/nucleo_f401re_pinout.md; a pin change edits this file
// and that table together.
//
// Pin names and ADC1 channels follow DS10086 Table 9; the internal channels follow RM0368 11.9
// (VREFINT on ADC1_IN17) and 11.10 (temperature sensor on ADC1_IN18).

use crate::bsp::ports::{PinId, PortId};
use crate::drivers::adc::{AdcMap, ExternalInput};
use crate::drivers::gpio::GpioMap;
use ipmc_sensors::board_sensors::{RAILS, RAIL_COUNT};
use ipmc_sensors::hal::{Channel, Divider, RailWiring};

/// pin -- a pin by port and number.
const fn pin(port: PortId, pin: u8) -> PinId {
    PinId { port, pin }
}

/// ADC -- ADC1 assignment. External input 0 is A0 (PA0, channel 0, rail "12V payload"), 1 is A1
/// (PA1, channel 1, "5V"), 2 is A2 (PA4, channel 4, "3V3 payload"), 3 is A3 (PB0, channel 8,
/// "3V3_AUX").
pub const ADC: AdcMap = AdcMap {
    vrefint_channel: 17,
    temperature_channel: 18,
    external: [
        ExternalInput {
            pin: pin(PortId::A, 0),
            adc_channel: 0,
        },
        ExternalInput {
            pin: pin(PortId::A, 1),
            adc_channel: 1,
        },
        ExternalInput {
            pin: pin(PortId::A, 4),
            adc_channel: 4,
        },
        ExternalInput {
            pin: pin(PortId::B, 0),
            adc_channel: 8,
        },
    ],
};

/// RAIL_WIRING -- which channel carries each rail of the sensor table (`ipmc_sensors::board_sensors::RAILS`,
/// same order) and the divider in front of it. Rail 0 is VDDA, measured from the internal
/// reference. Rails 1..4 are external inputs 0..3 (`ADC.external`): 12V payload on A0 behind the
/// defs.rs IPMC_RAIL1_DIV divider, 5V on A1, 3V3 payload on A2 and 3V3_AUX on A3 behind
/// IPMC_RAIL2..4_DIV. The sensor table and the SDR records are board neutral; a board that
/// wires a rail differently edits this table only.
pub const RAIL_WIRING: [RailWiring; RAIL_COUNT] = [
    RailWiring {
        channel: Channel::Vdda,
        divider: Divider { num: 1, den: 1 },
    },
    RailWiring {
        channel: Channel::Adc(0),
        divider: Divider {
            num: ipmc_defs::IPMC_RAIL1_DIV_NUM,
            den: ipmc_defs::IPMC_RAIL1_DIV_DEN,
        },
    },
    RailWiring {
        channel: Channel::Adc(1),
        divider: Divider {
            num: ipmc_defs::IPMC_RAIL2_DIV_NUM,
            den: ipmc_defs::IPMC_RAIL2_DIV_DEN,
        },
    },
    RailWiring {
        channel: Channel::Adc(2),
        divider: Divider {
            num: ipmc_defs::IPMC_RAIL3_DIV_NUM,
            den: ipmc_defs::IPMC_RAIL3_DIV_DEN,
        },
    },
    RailWiring {
        channel: Channel::Adc(3),
        divider: Divider {
            num: ipmc_defs::IPMC_RAIL4_DIV_NUM,
            den: ipmc_defs::IPMC_RAIL4_DIV_DEN,
        },
    },
];

// The wiring table has one entry per rail of the sensor table: a rail without wiring (or wiring
// without a rail) fails the build instead of reading a wrong channel.
const _: () = assert!(
    RAIL_WIRING.len() == RAILS.len(),
    "RAIL_WIRING and the sensor table RAILS disagree in length"
);

/// GPIO -- the digital signals: PWR_EN on PB5 (D4) and PAYLOAD_RST_OUT_N on PA9 (D8, CN5-1 and
/// CN10-21, open drain, active low); PGOOD on PB10 (D6), FAULT_N on PA10 (D2), OVERTEMP_N on PC7
/// (D9) and PAYLOAD_RESET_N on PA6 (D12, CN5-5, CN10-13), in `Input::index` order.
pub const GPIO: GpioMap = GpioMap {
    power_enable: pin(PortId::B, 5),
    inputs: [
        pin(PortId::B, 10),
        pin(PortId::A, 10),
        pin(PortId::C, 7),
        pin(PortId::A, 6),
    ],
    payload_reset_out: pin(PortId::A, 9),
};

// drivers/board_pins.rs - the board's pin and ADC channel assignments for the sensors: the one place
// that says which MCU pin carries which sensor signal. The stm32f401 drivers in ipmc-sensors take
// these tables at construction and hold no board pin of their own; board_sensors.rs refers to
// the external rails by abstract input number (entry n of `ADC.external`). The same assignment
// is listed for people in Documentation/nucleo_f401re_pinout.md; a pin change edits this file
// and that table together.
//
// Pin names and ADC1 channels follow DS10086 Table 9; the internal channels follow RM0368 11.9
// (VREFINT on ADC1_IN17) and 11.10 (temperature sensor on ADC1_IN18).

use ipmc_sensors::drivers::stm32f401::adc::{AdcMap, ExternalInput};
use ipmc_sensors::drivers::stm32f401::gpio::GpioMap;
use ipmc_sensors::drivers::stm32f401::ports::{PinId, PortId};

/// pin -- a pin by port and number.
const fn pin(port: PortId, pin: u8) -> PinId {
    PinId { port, pin }
}

/// ADC -- ADC1 assignment. External input 0 is A0 (PA0, channel 0, rail "12V payload"), 1 is A1
/// (PA1, channel 1, "5V"), 2 is A2 (PA4, channel 4, "3V3 payload"), 3 is A3 (PB0, channel 8,
/// "+3.3V_AUX").
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

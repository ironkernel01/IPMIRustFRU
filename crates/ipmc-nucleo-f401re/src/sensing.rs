// sensing.rs - which sensor hardware this firmware build needs.
//
// The sensor service is `ipmc_runtime::sensing::Sensing`, the SENSORS, POWER and SIM handlers are
// ipmc-runtime (`sensing_cmds`); both are generic over the board. This module says which on-chip
// hardware the ipmc-defs source switches ask for (`hardware_needs`), which main.rs starts.

use ipmc_sensors::routing::{HardwareNeeds, Source};

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

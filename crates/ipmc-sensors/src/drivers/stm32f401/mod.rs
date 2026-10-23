//! stm32f401 - on-chip drivers of the STM32F401RE, behind the Cargo feature `stm32f401`:
//! `adc` (ADC1: VREFINT, die temperature, the external rail inputs) implements `AnalogSource`; `gpio`
//! (the power enable and the four inputs) implements `DigitalInputs` and `PowerSwitch`; `ports` owns GPIOA..GPIOC
//! and hands out claimed pins to every module.
//!
//! Adding another on-chip part (another STM32) means a sibling directory with the same two
//! files and its own feature; the `IPMC_SRC_HW` arm of routing.rs and the firmware pick it.

pub mod adc;
pub mod gpio;
pub mod ports;

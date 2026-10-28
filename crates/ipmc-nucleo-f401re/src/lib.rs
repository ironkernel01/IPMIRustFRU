// ipmc_nucleo_f401re (library) - the board support of the NUCLEO-F401RE that both images use: the
// runtime firmware (src/main.rs, with its own `drivers` for the IPMB and the FRU store) and the
// bootloader (sectors 0-1). Holds only what both need; anything only the runtime uses lives in the
// runtime binary's tree, so the bootloader never links its interrupt handlers. The handlers of
// this library (`USART1`, `USART2`, `SysTick`, `HardFault`, `DefaultHandler`) reach the vector
// table of each binary that links it. No `core::fmt` is reachable from the always-linked code
// (the fault handlers write fixed text and `ipmc_boot::text` numbers).

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod bsp;

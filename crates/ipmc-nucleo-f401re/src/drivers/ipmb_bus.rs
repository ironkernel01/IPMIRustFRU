// drivers/ipmb_bus.rs - the F401 implementation of `ipmc_hal::ipmb::IpmbBus`: both IPMB buses, the
// master method of ipmc-defs IPMB_MASTER_MODE and the two I2C slaves.
//
// Ownership: `F401Ipmb` holds the master (`SelectedMaster`: the IPMB pins, the cycle table and
// the counters), the clock and interrupt controllers the I2C slave start needs, and the I2C
// peripherals until a bus starts. It adds no behaviour: each method calls the module that owns
// the work (the master's trait for the recovery, the write and the fault clear, `i2c_hw` for the
// slave start, the suspend and the counters). The link layer (ipmc-runtime) is written once over
// the trait.

use cortex_m::peripheral::{DCB, DWT, NVIC};
use ipmc_core::link::BusSpeed;
use ipmc_core::queue::Bus;
use ipmc_hal::ipmb::{BringUpFailure, DriverStats, IpmbBus, MasterError, MasterStats, Recovery};
use stm32f4::stm32f401::{I2C1, I2C3, RCC};

use super::i2c_hw::{self, I2cPeripheral, RxConsumer};
use super::ipmb_common::TimebaseError;
use super::ipmb_master::{self, IpmbMaster, SelectedMaster};
use super::ipmb_pins::IpmbPins;

/// B_FITTED -- IPMB-B is built in (defs.rs IPMB_BUS_B_ENABLE).
const B_FITTED: bool = ipmc_defs::IPMB_BUS_B_ENABLE;

/// BUSES -- one peripheral slot per bus.
const BUSES: usize = 2;

/// BringUp -- what `bring_up` takes besides the bus: the clock and interrupt controllers and the
/// PCLK1 the I2C timing derives from. Owned by `F401Ipmb`, so a bus that failed at start-up can be
/// brought up again.
struct BringUp {
    rcc: RCC,
    nvic: NVIC,
    pclk1_hz: u32,
}

/// DriverParts -- the shared peripherals and clocks `F401Ipmb::new` uses.
pub struct DriverParts<'a> {
    pub rcc: RCC,
    pub nvic: NVIC,
    pub dcb: &'a mut DCB,
    pub dwt: &'a mut DWT,
    /// Core clock (HCLK) in Hz: the DWT cycle counter runs at it.
    pub hclk_hz: u32,
    pub pclk1_hz: u32,
}

/// F401Ipmb -- both IPMB buses of the board.
pub struct F401Ipmb {
    master: SelectedMaster,
    bring_up: BringUp,
    /// The I2C peripheral of each bus until its `bring_up` succeeds (None: running, or not fitted).
    periph: [Option<I2cPeripheral>; BUSES],
}

impl F401Ipmb {
    /// new -- the master (IPMB_MASTER_MODE) takes the pins and the DWT cycle counter; an Err here
    /// means the board cannot transmit and the caller stops. IPMB-B not fitted: I2C3 is dropped
    /// unused and its pins stay parked.
    pub fn new(
        parts: DriverParts<'_>,
        i2c1: I2C1,
        i2c3: I2C3,
        pins: IpmbPins,
    ) -> Result<Self, TimebaseError> {
        let master =
            ipmb_master::new_selected(&mut *parts.dcb, &mut *parts.dwt, parts.hclk_hz, pins)?;
        Ok(F401Ipmb {
            master,
            bring_up: BringUp {
                rcc: parts.rcc,
                nvic: parts.nvic,
                pclk1_hz: parts.pclk1_hz,
            },
            periph: [
                Some(I2cPeripheral::I1(i2c1)),
                B_FITTED.then_some(I2cPeripheral::I3(i2c3)),
            ],
        })
    }
}

impl IpmbBus for F401Ipmb {
    type Rx = RxConsumer;

    /// bring_up -- bus recovery if SDA is held low, then the I2C slave. On error the peripheral
    /// stays in `periph` for the next call.
    fn bring_up(
        &mut self,
        bus: Bus,
        own_addr8: u8,
    ) -> Result<(Self::Rx, Recovery), BringUpFailure> {
        let recovery = self
            .master
            .recover_if_stuck(bus)
            .map_err(BringUpFailure::Bus)?;
        let ctx = &mut self.bring_up;
        i2c_hw::init(
            bus,
            &mut self.periph[bus.index()],
            &ctx.rcc,
            &mut ctx.nvic,
            self.master.pins(bus),
            ctx.pclk1_hz,
            own_addr8,
        )
        .map(|rx| (rx, recovery))
        .map_err(|error| BringUpFailure::Init(error.name()))
    }

    fn write(&mut self, bus: Bus, frame: &[u8]) -> Result<(), MasterError> {
        self.master.write(bus, frame)
    }

    fn set_speed(&mut self, bus: Bus, speed: BusSpeed) -> bool {
        self.master.set_speed(bus, speed)
    }

    fn lines_high(&self, bus: Bus) -> bool {
        self.master.lines_high(bus)
    }

    fn clear_bus_fault(&mut self, bus: Bus) -> Result<(), MasterError> {
        self.master.clear_bus_fault(bus)
    }

    fn suspend(&mut self, bus: Bus) -> bool {
        i2c_hw::suspend(bus)
    }

    fn suspend_faulted(&mut self, bus: Bus) -> bool {
        i2c_hw::suspend_faulted(bus)
    }

    fn resume(&mut self, bus: Bus) {
        i2c_hw::resume(bus, self.master.pins(bus));
    }

    fn slave_stats(&self, bus: Bus) -> DriverStats {
        i2c_hw::stats(bus)
    }

    fn master_stats(&self) -> &MasterStats {
        self.master.stats()
    }
}

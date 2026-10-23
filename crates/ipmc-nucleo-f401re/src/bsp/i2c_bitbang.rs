// bsp/i2c_bitbang.rs - the bit-bang IPMB master (the PIC32 method): the I2C peripheral is switched
// off, the same pins are driven open-drain as GPIO, then the peripheral is switched back on.
//
// Ownership: this module owns the IPMB pins (the `IpmbPins` handles from bsp/ipmb_pins.rs) while
// it is the master, and the DWT cycle counter use (the peripherals are taken in `main` and passed
// to `BitbangMaster::new`). It reaches the I2C registers only through bsp/i2c_hw.rs
// (`slave_idle`, `suspend`, `resume`).
// Method: PLAN 3.4 and i2c-bitbang.md. Open-drain on both lines: a line is released (output
// latch high) or driven low (latch low); a pin is never driven high. The half-bit delay and
// every wait come from the DWT cycle counter. No logging inside a transfer.

use cortex_m::peripheral::{DCB, DWT};
use ipmc_core::queue::Bus;
use ipmc_sensors::drivers::stm32f401::ports::{Pins, Pull, Speed};

use super::i2c_hw;
use super::ipmb_common::{
    cycles_for, now, wait_bus_free, wait_for, BusTimes, Deadline, TimebaseError,
};
use super::ipmb_master::{IpmbMaster, MasterError, MasterState, Recovery};
use super::ipmb_pins::{BusPins, IpmbPins};

/// T_HIGH_MIN_NS -- SCL high time, standard mode: at least 4.0 us (I2C-bus specification Table 5,
/// tHIGH). The high phase ends at least this long after SCL was seen high.
const T_HIGH_MIN_NS: u32 = 4_000;
/// T_SU_DAT_NS -- data setup time, standard mode: SDA stable at least 250 ns before SCL rises
/// (I2C-bus specification Table 10, tSU;DAT). Timed from the SDA write itself, so an interrupt
/// between the low-phase start and the SDA write cannot shorten it.
const T_SU_DAT_NS: u32 = 250;
/// RECOVER_CLOCKS_MAX -- SCL pulses a recovery sends to let a slave finish a byte it believes is
/// in progress: at most 8 data bits plus the ACK clock (i2c-bitbang.md section 3).
const RECOVER_CLOCKS_MAX: u32 = 9;
/// Clock bits per byte on the wire: eight data bits and the ACK clock.
const BITS_PER_BYTE: u32 = 9;
/// Cycles -- the times of the master in core clock cycles.
#[derive(Clone, Copy)]
pub(super) struct Cycles {
    /// Half a bit: IPMB_BITBANG_HALF_NS.
    half: u32,
    /// Slack of the high phase: half a bit less T_HIGH_MIN_NS. A rise (plus the poll that sees
    /// it) up to this long is absorbed inside the half bit; a longer wait (a stretched clock)
    /// extends the phase so SCL stays high for T_HIGH_MIN_NS after it was seen high.
    rise_allow: u32,
    /// T_SU_DAT_NS in cycles.
    su_dat: u32,
    /// The waits both masters share (stretch, bus free, ...).
    t: BusTimes,
}

impl Cycles {
    /// times -- the waits both masters share.
    pub(super) const fn times(&self) -> BusTimes {
        self.t
    }

    /// new -- the times at `core_hz`; None when the clock is zero, a time overflows 32 bits, or
    /// a half bit rounds to zero cycles.
    pub(super) fn new(core_hz: u32) -> Option<Cycles> {
        let half = cycles_for(u64::from(ipmc_defs::IPMB_BITBANG_HALF_NS), core_hz)?;
        let high_min = cycles_for(u64::from(T_HIGH_MIN_NS), core_hz)?;
        if half == 0 {
            return None;
        }
        Some(Cycles {
            half,
            rise_allow: half.saturating_sub(high_min),
            su_dat: cycles_for(u64::from(T_SU_DAT_NS), core_hz)?,
            t: BusTimes::new(core_hz)?,
        })
    }
}

/// Wire -- the two lines of one bus and the pacing of one transfer. Open-drain primitives only.
struct Wire<'a> {
    scl: &'a Pins,
    sda: &'a Pins,
    cy: Cycles,
    /// Cycle count at which the current phase ends; moved by `wait_half`.
    next: u32,
    /// Cycle count at the start of the transfer; every clock release must end within
    /// `cy.t.frame` of it (VITA 46.11 Table 4.1.1-1 T1, the bound the hardware master uses too).
    started: u32,
    /// The write's shared time budget (T2 + T1 from the call, all arbitration retries included).
    deadline: Deadline,
    /// Cycles from the first SCL fall to the last ACK clock of the latest completed transfer.
    bit_cycles: u32,
    /// Clock bits in `bit_cycles`.
    bits: u32,
}

impl<'a> Wire<'a> {
    fn new(pins: &'a BusPins, cy: Cycles, deadline: Deadline) -> Self {
        Wire {
            scl: &pins.scl,
            sda: &pins.sda,
            cy,
            next: now(),
            started: now(),
            deadline,
            bit_cycles: 0,
            bits: 0,
        }
    }

    // Line primitives. `set_all(true)` sets the output latch of an open-drain pin: released.
    fn release_scl(&self) {
        self.scl.set_all(true);
    }
    fn drive_scl_low(&self) {
        self.scl.set_all(false);
    }
    fn release_sda(&self) {
        self.sda.set_all(true);
    }
    fn drive_sda_low(&self) {
        self.sda.set_all(false);
    }
    fn scl_high(&self) -> bool {
        self.scl.read() != 0
    }
    fn sda_high(&self) -> bool {
        self.sda.read() != 0
    }
    fn release_both(&self) {
        self.release_scl();
        self.release_sda();
    }

    /// rebase -- starts the next phase from now.
    fn rebase(&mut self) {
        self.next = now();
    }

    /// wait_half -- ends the current phase one half bit after the previous phase ended, so the
    /// time spent in pin operations does not add to the bit time. A phase that ran more than a
    /// half bit late (an interrupt came in) resyncs to now. The wait is at most two half bits:
    /// the phase end is never more than one half bit ahead of the cycle counter.
    fn wait_half(&mut self) {
        let half = self.cy.half;
        self.next = self.next.wrapping_add(half);
        let entered = now();
        loop {
            let t = now();
            let late = t.wrapping_sub(self.next) as i32;
            if late >= 0 {
                if late.unsigned_abs() > half {
                    self.next = t;
                }
                return;
            }
            if t.wrapping_sub(entered) > half.saturating_mul(2) {
                self.next = t; // the counter is not where it should be: leave the wait
                return;
            }
        }
    }

    /// hold_since -- returns once `cycles` have passed since `t0`. Bounded by `cycles` polls (each
    /// poll takes at least one cycle), so a cycle counter that does not advance cannot hang it.
    fn hold_since(&self, t0: u32, cycles: u32) {
        for _ in 0..cycles {
            if now().wrapping_sub(t0) >= cycles {
                return;
            }
        }
    }

    /// release_scl_wait -- releases SCL and waits for it to read high, bounded by
    /// IPMB_STRETCH_TIMEOUT_US (a slave may stretch it), by T1 for the whole transfer and by the
    /// write's shared deadline. The
    /// high phase then ends one half bit after the release, or T_HIGH_MIN_NS after SCL was seen
    /// high when that is later (a slow rise or a stretch), so the SCL period stays at two half
    /// bits plus the time the pin operations take.
    fn release_scl_wait(&mut self) -> Result<(), MasterError> {
        let released_at = now();
        self.release_scl();
        let seen = wait_for(
            || self.scl_high(),
            &[
                (released_at, self.cy.t.stretch),
                (self.started, self.cy.t.frame),
                self.deadline.as_pair(),
            ],
        );
        if !seen {
            return Err(MasterError::StretchTimeout);
        }
        let seen_after = now().wrapping_sub(released_at);
        self.next = released_at.wrapping_add(seen_after.saturating_sub(self.cy.rise_allow));
        Ok(())
    }

    /// start -- START condition from released lines: SDA falls while SCL is high, then SCL falls.
    /// Setup (tSU;STA) and hold (tHD;STA) are one half bit each, at least 4.7 us and 4.0 us.
    /// The hold is timed from the cycle count read right after SDA is driven low (`rebase`), so an
    /// interrupt before that point cannot shorten it below one half bit (UM10204 Table 10,
    /// tHD;STA >= 4.0 us). Setup needs no such rule: it only gets longer when delayed.
    fn start(&mut self) {
        self.rebase();
        self.wait_half();
        self.drive_sda_low();
        self.rebase();
        self.wait_half();
        self.drive_scl_low();
        self.rebase();
    }

    /// bit -- one clock with `level` on SDA; returns the SDA level sampled at the end of the SCL
    /// high phase. SCL is low on entry and on exit.
    /// 1. SDA to `level` (released for 1) while SCL is low; low period one half bit, and at
    ///    least tSU;DAT from the SDA write itself.
    /// 2. SCL released, wait for it to read high (stretch).
    /// 3. Sample SDA as soon as SCL is high (the data has been stable for the whole low phase,
    ///    longer than the slave's ACK delay of 3.45 us), then finish the high period.
    /// 4. SCL low; the low phase restarts from now.
    fn bit(&mut self, level: bool) -> Result<bool, MasterError> {
        if level {
            self.release_sda();
        } else {
            self.drive_sda_low();
        }
        let sda_set = now();
        self.wait_half();
        self.hold_since(sda_set, self.cy.su_dat);
        self.release_scl_wait()?;
        let sampled = self.sda_high();
        self.wait_half();
        self.drive_scl_low();
        self.rebase();
        Ok(sampled)
    }

    /// byte -- eight bits MSB first, then the ACK clock with SDA released. After every released
    /// 1 bit SDA is read: low means another master pulls it, ArbitrationLost. Ok(true) is an
    /// ACK (the slave pulled SDA low), Ok(false) a NACK.
    fn byte(&mut self, value: u8) -> Result<bool, MasterError> {
        for shift in (0..8).rev() {
            let level = (value >> shift) & 1 == 1;
            let sampled = self.bit(level)?;
            if level && !sampled {
                return Err(MasterError::ArbitrationLost);
            }
        }
        let sda_at_ack = self.bit(true)?;
        Ok(!sda_at_ack)
    }

    /// stop -- STOP condition from SCL low: SDA low, SCL released, SDA released, then the bus
    /// free time (a half bit, at least 4.7 us, tBUF). tSU;STO is timed from the SCL release by
    /// `release_scl_wait`; tBUF from the cycle count read right after SDA is released (`rebase`),
    /// so an interrupt before the release cannot shorten it.
    fn stop(&mut self) -> Result<(), MasterError> {
        self.drive_sda_low();
        self.wait_half();
        let scl = self.release_scl_wait();
        self.wait_half();
        self.release_sda();
        self.rebase();
        self.wait_half();
        scl
    }

    /// recover_sda -- clocks SCL up to RECOVER_CLOCKS_MAX times until a slave releases SDA, then
    /// sends a STOP to reset the slave state (i2c-bitbang.md section 3, bus_recover). True when
    /// SDA reads high afterwards.
    fn recover_sda(&mut self) -> bool {
        self.rebase();
        for _ in 0..RECOVER_CLOCKS_MAX {
            if self.sda_high() {
                break;
            }
            self.drive_scl_low();
            self.wait_half();
            if self.release_scl_wait().is_err() {
                break;
            }
            self.wait_half();
        }
        self.drive_scl_low();
        self.wait_half();
        self.drive_sda_low();
        self.wait_half();
        let _ = self.release_scl_wait();
        self.wait_half();
        self.release_sda();
        self.rebase(); // tBUF from the actual release, as in `stop`
        self.wait_half();
        self.sda_high()
    }

    /// send -- START, the address byte `frame[0]`, the data bytes `frame[1..]`, STOP. The time
    /// from the first SCL fall to the last ACK clock is kept for the SCL period report.
    fn send(&mut self, frame: &[u8]) -> Result<(), MasterError> {
        self.started = now();
        self.start();
        let first_fall = now();
        let mut last_ack_end = first_fall;
        for (index, &value) in frame.iter().enumerate() {
            if !self.byte(value)? {
                return Err(if index == 0 {
                    MasterError::AddressNack
                } else {
                    MasterError::DataNack
                });
            }
            last_ack_end = now();
        }
        self.bit_cycles = last_ack_end.wrapping_sub(first_fall);
        self.bits = u32::try_from(frame.len())
            .unwrap_or(u32::MAX)
            .saturating_mul(BITS_PER_BYTE);
        self.stop()
    }

    /// verify_released -- after a transfer that ended with a STOP (or tried to): both lines
    /// must read high. SDA low gets the recovery clocks.
    fn verify_released(&mut self) -> Result<(), MasterError> {
        self.release_both();
        if !self.scl_high() {
            return Err(MasterError::SclStuckLow);
        }
        if !self.sda_high() {
            // EVIDENCE: SDA reads low after the STOP; a slave holds it in the middle of a byte.
            if !self.recover_sda() {
                return Err(MasterError::SdaStuckLow);
            }
        }
        Ok(())
    }

    /// transfer -- one master write on the released, idle lines, then the cleanup for the way
    /// it ended.
    /// - ArbitrationLost, StretchTimeout: release both lines at once; another master or a slave
    ///   owns the bus.
    /// - AddressNack, DataNack: STOP, then verify the lines.
    /// - Ok: verify the lines; a stuck line is the result.
    fn transfer(&mut self, frame: &[u8]) -> Result<(), MasterError> {
        let outcome = self.send(frame);
        match outcome {
            Err(error @ (MasterError::ArbitrationLost | MasterError::StretchTimeout)) => {
                self.release_both();
                Err(error)
            }
            Err(error @ (MasterError::AddressNack | MasterError::DataNack)) => {
                self.drive_scl_low();
                self.rebase();
                let _ = self.stop();
                self.verify_released().and(Err(error))
            }
            other => {
                let verified = self.verify_released();
                other.and(verified)
            }
        }
    }
}

/// recover_if_stuck -- before the I2C slave is enabled (the pins are parked inputs): if SDA
/// reads low, clocks SCL up to 9 times as open-drain GPIO and sends a STOP, then parks the
/// pins as inputs again (i2c-bitbang.md sections 3 and 5). Both masters use it at start-up.
/// Ok(NotNeeded): both lines high. Ok(Recovered): SDA was low and is free. Err(SclStuckLow):
/// SCL reads low, no clock can be sent. Err(SdaStuckLow): SDA still low after the clocks.
pub(super) fn recover_if_stuck(pins: &BusPins, cy: Cycles) -> Result<Recovery, MasterError> {
    let mut wire = Wire::new(pins, cy, Deadline::from_now(cy.t.write_budget));
    if !wire.scl_high() {
        return Err(MasterError::SclStuckLow);
    }
    if wire.sda_high() {
        return Ok(Recovery::NotNeeded);
    }
    // EVIDENCE: SDA reads low with the slave not yet running; a device holds it mid-byte.
    pins.scl.output(true, true, Speed::Medium);
    pins.sda.output(true, true, Speed::Medium);
    let freed = wire.recover_sda();
    pins.scl.input(Pull::None);
    pins.sda.input(Pull::None);
    if freed {
        Ok(Recovery::Recovered)
    } else {
        Err(MasterError::SdaStuckLow)
    }
}

/// BitbangMaster -- the bit-bang implementation of `IpmbMaster`.
pub struct BitbangMaster {
    state: MasterState,
}

impl BitbangMaster {
    /// new -- the shared master state (`MasterState::new`) with the bit-bang transfer.
    pub fn new(
        dcb: &mut DCB,
        dwt: &mut DWT,
        core_hz: u32,
        pins: IpmbPins,
    ) -> Result<Self, TimebaseError> {
        MasterState::new(dcb, dwt, core_hz, pins).map(|state| BitbangMaster { state })
    }
}

impl IpmbMaster for BitbangMaster {
    fn state(&self) -> &MasterState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut MasterState {
        &mut self.state
    }

    /// write_once -- one attempt: wait for a free bus and suspend the slave, switch the pins to
    /// open-drain GPIO (latch high: released), transfer, then give the bus back to the slave.
    fn write_once(
        &mut self,
        bus: Bus,
        frame: &[u8],
        deadline: Deadline,
    ) -> Result<(), MasterError> {
        let state = &mut self.state;
        let pins = state.pins.bus(bus);
        let mut wire = Wire::new(pins, state.cy, deadline);
        wait_bus_free(bus, pins, &state.cy.t, deadline, i2c_hw::suspend)?;
        // While PE = 0 the slave cannot hear its bus: a frame another master sends from here to
        // `resume` is lost and is recovered by the requester's retry (PLAN 3.4).
        pins.scl.output(true, true, Speed::Medium);
        pins.sda.output(true, true, Speed::Medium);
        let result = if wire.scl_high() && wire.sda_high() {
            wire.transfer(frame)
        } else {
            // Another master started inside the window above; stay off the bus and retry after
            // its frame (arbitration class, not a bus fault).
            Err(MasterError::ArbitrationLost)
        };
        i2c_hw::resume(bus, pins);
        if result.is_ok() {
            state.stats.last_bit_cycles = wire.bit_cycles;
            state.stats.last_bits = wire.bits;
        }
        result
    }
}

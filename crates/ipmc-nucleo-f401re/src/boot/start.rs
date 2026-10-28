// start.rs - the start sequence: the bootloader leaves the processor as a reset leaves it, for
// every part it touched, then branches to the runtime reset handler (Documentation/
// ipmc_boot_plan.md 6.4, steps 2 to 8). The runtime start-up order (main.rs) then runs
// unchanged. The same sequence serves every start (after the checks, after the countdown).
//
// Ownership: this module owns the hand-over. It is the last code the bootloader runs: it takes
// the peripherals again (`steal`) because every owner (the UART driver, the LED, SysTick, the
// flash layer) is finished by then, and it never returns.

use cortex_m::peripheral::{NVIC, SCB, SYST};
use ipmc_nucleo_f401re::bsp::{clock, cycles};
use stm32f4::stm32f401;

extern "C" {
    /// The runtime vector table: `__app_vectors` of flash_map.x, whose first two words are the
    /// initial stack pointer and the reset vector.
    static __app_vectors: [u32; 2];
}

/// NVIC_WORDS -- 32-bit words of the NVIC enable and pending registers that hold the 85
/// interrupts of the STM32F401 (RM0368 Table 38): ceil(85 / 32).
const NVIC_WORDS: usize = 3;

/// start_runtime -- steps 3 to 8 of design 6.4 (step 2, the flush of the console and of APP_UART
/// when up, is `ipmc_boot::run`'s, done before this call):
/// 3. mask interrupts, stop SysTick;
/// 4. reset USART1, USART2, PWR and GPIOA..C through RCC (each pulsed set then clear), put
///    their clock enables back to the reset values, then clear every NVIC enable and pending
///    bit (after the peripheral reset, so an IRQ latched while a USART still ran is cleared too);
/// 5. `clock::restore_reset_state`; a fault there does not stop the start, the runtime
///    `clock::init` reports a PLL it finds running as its clock fault;
/// 6. DWT cycle counter off;
/// 7. VTOR = `__app_vectors`;
/// 8. MSP from word 0, CONTROL cleared, `cpsie i`, branch to word 1.
///
/// The caller has checked the image (`ipmc_boot::header::check`), so the vector table holds a
/// valid stack pointer and a Thumb reset vector inside the image.
pub fn start_runtime() -> ! {
    cortex_m::interrupt::disable();

    // SAFETY: interrupts are masked and nothing runs after this function but the runtime: every
    // module that owned a peripheral is finished, so a second handle cannot race a first.
    let (dp, mut cp) = unsafe {
        (
            stm32f401::Peripherals::steal(),
            cortex_m::Peripherals::steal(),
        )
    };

    // Step 3: SysTick stopped (CTRL = 0) and not pending; interrupts stay masked.
    // SAFETY: SYST_CSR = 0 stops the counter and its interrupt; nothing else uses SysTick now.
    unsafe { (*SYST::PTR).csr.write(0) };
    SCB::clear_pendst();
    SCB::clear_pendsv();

    // Step 4: the peripherals the bootloader used, back to their reset state.
    let rcc = &dp.RCC;
    rcc.apb2rstr().modify(|_, w| w.usart1rst().reset());
    rcc.apb1rstr()
        .modify(|_, w| w.usart2rst().reset().pwrrst().reset());
    rcc.ahb1rstr()
        .modify(|_, w| w.gpioarst().reset().gpiobrst().reset().gpiocrst().reset());
    rcc.ahb1rstr().modify(|_, w| {
        w.gpioarst()
            .clear_bit()
            .gpiobrst()
            .clear_bit()
            .gpiocrst()
            .clear_bit()
    });
    rcc.apb1rstr()
        .modify(|_, w| w.usart2rst().clear_bit().pwrrst().clear_bit());
    rcc.apb2rstr().modify(|_, w| w.usart1rst().clear_bit());
    rcc.ahb1enr().reset();
    rcc.apb1enr().reset();
    rcc.apb2enr().reset();
    // Step 4, last part: no interrupt enabled or pending, now that no peripheral can raise one.
    for index in 0..NVIC_WORDS {
        // SAFETY: ICER and ICPR are write-one-to-clear registers; the writes only disable and
        // un-pend interrupts, with interrupts masked.
        unsafe {
            (*NVIC::PTR).icer[index].write(u32::MAX);
            (*NVIC::PTR).icpr[index].write(u32::MAX);
        }
    }

    // Step 5: the clock tree. A failure is not fatal here (see above).
    let _ = clock::restore_reset_state(rcc, &dp.FLASH);

    // Step 6: the runtime starts the cycle counter itself.
    cycles::stop_cycle_counter(&mut cp.DCB, &mut cp.DWT);

    // Step 7: VTOR to the runtime table.
    let vectors = core::ptr::addr_of!(__app_vectors);
    // SAFETY: `__app_vectors` is the 512-byte aligned start of APP (flash_map.x), where the
    // runtime vector table is (link.x asserts it; the image check read it); only the address of
    // the static is used here.
    unsafe { cp.SCB.vtor.write(vectors as u32) };
    cortex_m::asm::dsb();
    cortex_m::asm::isb();

    // Step 8: initial stack pointer and reset vector from the table.
    // SAFETY: the image check accepted the table: word 0 is an 8-byte aligned address in RAM and
    // word 1 an odd (Thumb) address inside the image; `__app_vectors` is declared as those two words
    // and both are read from the mapped flash.
    let (stack, reset) = unsafe {
        let words = vectors.cast::<u32>();
        (words.read_volatile(), words.add(1).read_volatile())
    };
    // SAFETY: loads MSP with the checked stack pointer, selects the main stack in thread mode
    // (CONTROL = 0, no FPU context), unmasks interrupts (the NVIC holds no enabled interrupt, so
    // none can fire before the runtime enables its own) and branches to the checked reset
    // vector. The runtime reset handler never returns, so this block never falls through.
    unsafe {
        core::arch::asm!(
            "msr msp, {stack}",
            "msr control, {zero}",
            "isb",
            "cpsie i",
            "bx {reset}",
            stack = in(reg) stack,
            reset = in(reg) reset,
            zero = in(reg) 0u32,
            options(noreturn, nostack)
        );
    }
}

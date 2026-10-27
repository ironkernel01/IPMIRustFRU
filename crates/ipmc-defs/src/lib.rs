//! ipmc-defs - the build configuration of the IPMC as Rust, with the compile-time checks on it.
//!
//! `defs.rs` is the configuration: one `pub const NAME: T = value;` per tunable, each with the
//! type its consumers need (`bool` for a switch, `usize` for a size or depth, a fixed-width
//! integer for a wire or register value, a small enum for a choice between alternatives, `&str`
//! for a text). There is no parser and no generated file. This file re-exports every item as
//! `ipmc_defs::NAME` and checks every value at compile time: a value that breaks a spec or
//! datasheet rule stops the build and names the define. When the private Tier-2/3 folder is
//! present, `build.rs` sets `cfg(tier23)` and this file mounts that folder's `defs_tier23.rs` and
//! `defs_guards.rs`; otherwise `IPMC_TIER` is 1. `build.rs` holds the one detection of the
//! extension and of the evaluation build.
#![no_std]
#![forbid(unsafe_code)]

mod defs;
pub use defs::*;

// The Tier-2/3 tunables, mounted only when the private folder is present (build.rs sets
// cfg(tier23)); the Tier-1 build reports Tier 1.
#[cfg(tier23)]
#[rustfmt::skip] // formatted by the Tier23 gate; absent, rustfmt cannot open it
#[path = "../../../../IPMIRustFRU_Tier23/defs_tier23.rs"]
mod defs_tier23;
#[cfg(tier23)]
pub use defs_tier23::*;
/// IPMC_TIER -- the Tier the IPMC reports in Get VSO Capabilities; the Tier-1 build has no
/// Tier-2/3 file and reports 1.
#[cfg(not(tier23))]
pub const IPMC_TIER: u32 = 1;

/// i2c_timing -- the IPMB timing arithmetic: cycles, SCL phases and peripheral divisors.
pub mod i2c_timing;

/// TIER23_BUILT -- true when build.rs found the Tier-2/3 extension folder (or IPMC_TIER23=1
/// demanded it) and mounted its defs_tier23.rs into these constants; false in the Tier-1 build.
/// The bridge crate `ipmc-ext` selects the extension from the same detection.
pub const TIER23_BUILT: bool = cfg!(tier23);

// The guards of the Tier-2/3 defines live with defs_tier23.rs in the extension folder.
#[cfg(tier23)]
#[rustfmt::skip] // formatted by the Tier23 gate; absent, rustfmt cannot open it
#[path = "../../../../IPMIRustFRU_Tier23/defs_guards.rs"]
mod tier23_guards;

const _: () = assert!(
    IPMC_TIER >= 1 && IPMC_TIER <= 3,
    "IPMC_TIER must be 1, 2 or 3 (VITA 46.11 section 4)"
);

const _: () = assert!(
    IPMB_GA_FORCED >= 1 && IPMB_GA_FORCED <= 0x1F,
    "IPMB_GA_FORCED must be 1..=0x1F (GA[4:0], 0 is invalid)"
);
// RAM of the STM32F401 for the linker (flash_map.x: 96 KB minus the 256 byte NOINIT block).
const RAM_BYTES: u32 = 96 * 1024 - 256;
const _: () = assert!(
    IPMC_STACK_RESERVE_BYTES.is_multiple_of(8) && IPMC_STACK_RESERVE_BYTES < RAM_BYTES,
    "IPMC_STACK_RESERVE_BYTES must be a multiple of 8 and below the RAM size"
);
const _: () = assert!(
    IPMC_BOOT_STACK_RESERVE_BYTES.is_multiple_of(8) && IPMC_BOOT_STACK_RESERVE_BYTES < RAM_BYTES,
    "IPMC_BOOT_STACK_RESERVE_BYTES must be a multiple of 8 and below the RAM size"
);
const _: () = assert!(IPMB_MSG_MAX == 32, "IPMB_MSG_MAX must be 32");
const _: () = assert!(
    IPMB_BUS_A_ENABLE,
    "IPMB_BUS_A_ENABLE must be 1 (VITA 46.11 Rule 4.1.1-1: IPMB-A is mandatory)"
);
const _: () = assert!(
    IPMB_CM_ADDR8 != 0 && IPMB_CM_ADDR8 <= 0xFE && IPMB_CM_ADDR8 & 1 == 0,
    "IPMB_CM_ADDR8 must be a nonzero 8-bit slave address with bit 0 clear"
);
// IPMB powers up at 100 kHz (VITA 46.11 Rule 4.1.1-4.3, standard mode); faster modes are
// negotiated later by the link layer, not configured here.
// STANDARD-MODE ceiling (VITA 46.11 Rule 4.1.1-4.3).
const _: () = assert!(
    IPMB_I2C_HZ >= 1 && IPMB_I2C_HZ <= 100_000,
    "IPMB_I2C_HZ must be at most 100000 (standard mode)"
);
const _: () = assert!(
    IPMB_I2C_HZ == 100_000,
    "IPMB_I2C_HZ must be 100000 (IPMB power-up speed)"
);
// Half bit time x 2 x clock = 1 s at the nominal rate; a shorter half bit would run the
// bit-bang master faster than IPMB_I2C_HZ. u128: the product of two u32 and 2 needs 65 bits.
const _: () = assert!(
    IPMB_BITBANG_HALF_NS as u128 * 2 * IPMB_I2C_HZ as u128 >= 1_000_000_000,
    "IPMB_BITBANG_HALF_NS is shorter than half a bit at IPMB_I2C_HZ"
);
// FAST-MODE (VITA 46.11 Rule 4.1.1-4.4): 400 kHz, the I2C fast-mode maximum. The hardware master
// derives its CCR from it with F/S = 1 and DUTY = 0 (RM0368 18.6.8).
const _: () = assert!(
    IPMB_I2C_FAST_HZ == 400_000,
    "IPMB_I2C_FAST_HZ must be 400000 (FAST-MODE)"
);
// The bit-bang master holds SCL low and high for at least the I2C fast-mode minimums, tLOW 1.3 us
// and tHIGH 0.6 us (VITA 46.11 Table 9.2.2-1 M0_P1b); the loop overhead and the rise time come
// on top of the two times, so the clock stays below 400 kHz.
const _: () = assert!(
    IPMB_BITBANG_FAST_LOW_NS >= 1_300 && IPMB_BITBANG_FAST_LOW_NS <= 100_000,
    "IPMB_BITBANG_FAST_LOW_NS must be 1300..=100000 (I2C fast-mode tLOW)"
);
const _: () = assert!(
    IPMB_BITBANG_FAST_HIGH_NS >= 600 && IPMB_BITBANG_FAST_HIGH_NS <= 100_000,
    "IPMB_BITBANG_FAST_HIGH_NS must be 600..=100000 (I2C fast-mode tHIGH)"
);
const _: () = assert!(
    IPMC_PAYLOAD_RESET_PULSE_MS >= 1 && IPMC_PAYLOAD_RESET_PULSE_MS <= 1_000,
    "IPMC_PAYLOAD_RESET_PULSE_MS must be 1..=1000"
);
// Payload modes (VITA 46.11 Table 10.2.8-1): modes 0..=14 are bits 0..=14; the normal mode 0
// is always supported and bit 15 is reserved.
const _: () = assert!(
    IPMC_PAYLOAD_MODES & 1 == 1 && IPMC_PAYLOAD_MODES >> 15 == 0,
    "IPMC_PAYLOAD_MODES must have bit 0 set and bits 15 and up clear"
);
// 8 entries is the least that rides out a few interleaved retries; 64 is 64 x 32-byte frames
// of RAM (about 3 KB with keys), the most this part should spend on the cache.
const _: () = assert!(
    IPMB_DUP_CACHE >= 8 && IPMB_DUP_CACHE <= 64,
    "IPMB_DUP_CACHE must be 8..=64"
);
// The requester retries for up to (C1max 10 + 1) x T3max 250 ms = 2750 ms (VITA 46.11 Table
// 4.1.1-1); a shorter window would let a late retry run twice. The VITA T9 sequence expiry (5 s)
// cannot be the window: the live Chassis Manager reuses every rqSeq after 3846 ms or more and
// repeats identical requests at 3883 ms (PLAN.md 3.3), so the window stays below 3500 ms.
const _: () = assert!(
    IPMB_DUP_WINDOW_MS >= 2750 && IPMB_DUP_WINDOW_MS <= 3500,
    "IPMB_DUP_WINDOW_MS must be 2750..=3500 (covers the 2750 ms requester retry window, stays below the 3846 ms sequence reuse measured on the live Chassis Manager)"
);
// VITA 46.11 Table 4.1.1-1 C1: the requester retries 3 to 10 times; the response resends follow
// the same bound (Rule 4.1.1-7).
const _: () = assert!(
    IPMB_TX_RETRIES >= 3 && IPMB_TX_RETRIES <= 10,
    "IPMB_TX_RETRIES must be 3..=10 (VITA 46.11 retry count C1)"
);
// VITA 46.11 Table 4.1.1-1 and Rules 4.1.1-9, 4.1.1-10: a request this IPMC sends waits T3max =
// 250 ms for its response, T6 spaces the retries (60..250 ms), the retry count C1 is 3..10 and
// successive new requests are at least T4 = 5 ms apart.
const _: () = assert!(
    IPMB_REQ_TIMEOUT_MS >= 60 && IPMB_REQ_TIMEOUT_MS <= 250,
    "IPMB_REQ_TIMEOUT_MS must be 60..=250 (VITA 46.11 Rule 4.1.1-9 T3max, Table 4.1.1-1 T6)"
);
const _: () = assert!(
    IPMB_REQ_RETRIES >= 3 && IPMB_REQ_RETRIES <= 10,
    "IPMB_REQ_RETRIES must be 3..=10 (VITA 46.11 Rule 4.1.1-10 retry count C1)"
);
const _: () = assert!(
    IPMB_REQ_GAP_MS >= 5,
    "IPMB_REQ_GAP_MS must be at least 5 (VITA 46.11 Table 4.1.1-1 T4)"
);
const _: () = assert!(
    IPMC_RESET_WAIT_MS >= 1 && IPMC_RESET_WAIT_MS <= 5_000,
    "IPMC_RESET_WAIT_MS must be 1..=5000"
);
const _: () = assert!(
    IPMB_INGRESS_DEPTH >= 1 && IPMB_INGRESS_DEPTH <= 32,
    "IPMB_INGRESS_DEPTH must be 1..=32"
);
// VITA 46.11 Table 4.1.1-1 T8 (I2C clock low hold) allows at most 3 ms.
const _: () = assert!(
    IPMB_STRETCH_TIMEOUT_US >= 1 && IPMB_STRETCH_TIMEOUT_US <= 3000,
    "IPMB_STRETCH_TIMEOUT_US must be 1..=3000 (VITA 46.11 Table 4.1.1-1 T8 max 3 ms)"
);
// T2 (bus free before a transmission) is at least 60 ms; waiting past T5 (227 ms) would miss the
// response deadline whatever the bus does.
const _: () = assert!(
    IPMB_BUS_FREE_TIMEOUT_MS >= 60 && IPMB_BUS_FREE_TIMEOUT_MS <= 227,
    "IPMB_BUS_FREE_TIMEOUT_MS must be 60..=227 (VITA 46.11 Table 4.1.1-1 T2 min, T5)"
);
const _: () = assert!(
    IPMB_RX_QUEUE_DEPTH >= 2 && IPMB_RX_QUEUE_DEPTH <= 16,
    "IPMB_RX_QUEUE_DEPTH must be 2..=16"
);
const _: () = assert!(IPMB_ARB_RETRIES <= 10, "IPMB_ARB_RETRIES must be 0..=10");
const _: () = assert!(
    IPMB_TXQ_DEPTH >= 1 && IPMB_TXQ_DEPTH <= 16,
    "IPMB_TXQ_DEPTH must be 1..=16"
);
// Rule 9.3-10: the local state changes only on a persistent error. At least 2 in a row makes it
// persistent; 64 keeps a fault from taking minutes to declare. The clear time is long enough to
// rule out a glitch and short enough for a prompt return (100 ms to 60 s).
const _: () = assert!(
    IPMB_FAULT_STREAK >= 2 && IPMB_FAULT_STREAK <= 64,
    "IPMB_FAULT_STREAK must be 2..=64"
);
const _: () = assert!(
    IPMB_FAULT_CLEAR_MS >= 100 && IPMB_FAULT_CLEAR_MS <= 60000,
    "IPMB_FAULT_CLEAR_MS must be 100..=60000"
);

// The capture log ring (ipmc-core capture.rs): 1 KiB holds a few frames, 16 KiB is the most the
// RAM budget below allows; a trace line is kept to IPMC_LOG_TEXT_MAX bytes, 32 holds a short
// status line, 200 the longest the console prints. The relations to IPMB_MSG_MAX are guarded
// beside the type (capture.rs).
// RAM budget: the log is 16 KB at most because the RAM of the STM32F401 is 96 KB and the statics
// (this log, the UART rings, the FRU image) share it with the stack. The firmware reserves 35 KB
// for the stack (`_stack_reserve` in memory.x, measured on the release ELF; the method and the
// numbers are in that file) and link.x fails the build when the statics leave less. The
// IpmbLink temporary in `take` was measured at 16.6 KB whatever the log size.
const _: () = assert!(
    IPMC_LOG_SIZE >= 1024 && IPMC_LOG_SIZE <= 16384 && IPMC_LOG_SIZE.is_power_of_two(),
    "IPMC_LOG_SIZE must be a power of two, 1024..=16384 (the usable size is one byte less)"
);
const _: () = assert!(
    IPMC_LOG_TEXT_MAX >= 32 && IPMC_LOG_TEXT_MAX <= 200,
    "IPMC_LOG_TEXT_MAX must be 32..=200"
);

// ---- console, LED and menu limits ----

// Highest baud the USART2 divider reaches at the highest PCLK1: 42 MHz / 16 (RM0368 19.3.4,
// 16x oversampling; PCLK1 is at most 42 MHz).
const USART2_BAUD_MAX: u32 = 2_625_000;
const _: () = assert!(
    IPMC_CONSOLE_BAUD >= 1 && IPMC_CONSOLE_BAUD <= USART2_BAUD_MAX,
    "IPMC_CONSOLE_BAUD must be 1..=2625000 (USART2 limit at PCLK1 = 42 MHz)"
);
// The RX ring indexes wrap with a mask (ipmc-nucleo-f401re src/bsp/uart.rs), so its size is a power of two; one slot
// stays empty, so a whole line must fit in size - 1 bytes.
const _: () = assert!(
    IPMC_UART_RX_BUF.is_power_of_two() && IPMC_UART_RX_BUF >= 2,
    "IPMC_UART_RX_BUF must be a power of two"
);
// The console TX ring is a ByteRing as well (power of two). Console output waits only while the
// ring is full, so it must hold the longest line the capture log keeps plus its CR LF; 256 bytes
// is the smallest ring that does. The application port has its own rate (USART1 is on PCLK2, at
// most 84 MHz: 84 MHz / 16 = 5.25 MBd), its flow control is a switch, and its rings are powers
// of two; the receive ring holds at least one 64-byte application line.
const USART1_BAUD_MAX: u32 = 5_250_000;
const _: () = assert!(
    IPMC_UART_TX_BUF.is_power_of_two()
        && IPMC_UART_TX_BUF >= 256
        && IPMC_UART_TX_BUF <= 16384
        && IPMC_UART_TX_BUF > IPMC_LOG_TEXT_MAX + 2,
    "IPMC_UART_TX_BUF must be a power of two, 256..=16384, above IPMC_LOG_TEXT_MAX + 2"
);
const _: () = assert!(
    IPMC_APP_UART_BAUD >= 1 && IPMC_APP_UART_BAUD <= USART1_BAUD_MAX,
    "IPMC_APP_UART_BAUD must be 1..=5250000 (USART1 limit at PCLK2 = 84 MHz)"
);
const _: () = assert!(
    IPMC_APP_UART_RX_BUF.is_power_of_two()
        && IPMC_APP_UART_RX_BUF >= 64
        && IPMC_APP_UART_RX_BUF <= 16384,
    "IPMC_APP_UART_RX_BUF must be a power of two, 64..=16384"
);
const _: () = assert!(
    IPMC_APP_UART_TX_BUF.is_power_of_two()
        && IPMC_APP_UART_TX_BUF >= 512
        && IPMC_APP_UART_TX_BUF <= 16384,
    "IPMC_APP_UART_TX_BUF must be a power of two, 512..=16384"
);
const _: () = assert!(
    IPMC_APP_LINE_MAX >= 16 && IPMC_APP_LINE_MAX + 2 < IPMC_APP_UART_RX_BUF,
    "IPMC_APP_LINE_MAX must be 16.. and, with CR LF, fit the usable IPMC_APP_UART_RX_BUF - 1"
);
const _: () = assert!(
    IPMC_APP_BYTES_PER_POLL >= 1 && IPMC_APP_BYTES_PER_POLL <= IPMC_APP_UART_RX_BUF,
    "IPMC_APP_BYTES_PER_POLL must be 1..=IPMC_APP_UART_RX_BUF"
);
// Text defines that reach a terminal. `printable_len_ok` checks 1..=max bytes of printable ASCII
// (20h..=7Eh). With `space_end` the last byte must be a space and earlier spaces are allowed
// (a prompt); without it no byte is a space (a name).
const fn printable_len_ok(text: &str, max: usize, space_end: bool) -> bool {
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes.len() > max || (space_end && bytes[bytes.len() - 1] != b' ') {
        return false;
    }
    let lowest = if space_end { 0x20 } else { 0x21 };
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] < lowest || bytes[i] > 0x7E {
            return false;
        }
        i += 1;
    }
    true
}
// A prompt of a request/answer port ends in a space, so a request echoed after it is one line that
// starts with the prompt.
const _: () = assert!(
    printable_len_ok(IPMC_APP_PROMPT, 16, true),
    "IPMC_APP_PROMPT must be 1..=16 printable ASCII bytes and end in a space"
);
const _: () = assert!(
    printable_len_ok(IPMC_BOOT_PROMPT, 16, true),
    "IPMC_BOOT_PROMPT must be 1..=16 printable ASCII bytes and end in a space"
);
const _: () = assert!(
    printable_len_ok(IPMC_CONSOLE_UART_NAME, 8, false),
    "IPMC_CONSOLE_UART_NAME must be 1..=8 printable ASCII bytes, no space"
);
const _: () = assert!(
    printable_len_ok(IPMC_APP_UART_NAME, 8, false),
    "IPMC_APP_UART_NAME must be 1..=8 printable ASCII bytes, no space"
);
const _: () = assert!(
    printable_len_ok(IPMC_CONSOLE_PROMPT, 16, false),
    "IPMC_CONSOLE_PROMPT must be 1..=16 printable ASCII bytes, no space"
);
const _: () = assert!(
    IPMC_MENU_LINE_MAX >= 1 && IPMC_MENU_LINE_MAX < IPMC_UART_RX_BUF,
    "IPMC_MENU_LINE_MAX must be 1..IPMC_UART_RX_BUF (usable ring bytes must cover a line)"
);
const _: () = assert!(
    IPMC_MENU_TOKENS_MAX >= 1 && IPMC_MENU_TOKENS_MAX <= IPMC_MENU_LINE_MAX,
    "IPMC_MENU_TOKENS_MAX must be 1..=IPMC_MENU_LINE_MAX"
);
const _: () = assert!(
    IPMC_CONSOLE_BYTES_PER_POLL >= 1 && IPMC_CONSOLE_BYTES_PER_POLL <= IPMC_UART_RX_BUF,
    "IPMC_CONSOLE_BYTES_PER_POLL must be 1..=IPMC_UART_RX_BUF"
);
// A FRU DUMP line is a 6-character offset and 3 characters per byte ("xx "): 16 bytes make 54
// characters, which fits the 80-column console; more than 16 is refused.
const _: () = assert!(
    IPMC_FRU_DUMP_BYTES_PER_LINE >= 1 && IPMC_FRU_DUMP_BYTES_PER_LINE <= 16,
    "IPMC_FRU_DUMP_BYTES_PER_LINE must be 1..=16"
);
const _: () = assert!(
    IPMC_LED_RATE_MIN_CHZ >= 1 && IPMC_LED_RATE_MIN_CHZ <= IPMC_LED_RATE_MAX_CHZ,
    "IPMC_LED_RATE_MIN_CHZ must be 1..=IPMC_LED_RATE_MAX_CHZ"
);
// The blink flasher runs from the 1 ms tick and toggles twice per flash, so 500 flashes per
// second (50 000 cHz) is the most it can produce.
const LED_RATE_CEILING_CHZ: u32 = 50_000;
const _: () = assert!(
    IPMC_LED_RATE_MAX_CHZ <= LED_RATE_CEILING_CHZ,
    "IPMC_LED_RATE_MAX_CHZ must be at most 50000 (500 Hz, the 1 ms tick toggling twice per flash)"
);
const _: () = assert!(
    IPMC_FAULT_BLINK_CHZ >= IPMC_LED_RATE_MIN_CHZ && IPMC_FAULT_BLINK_CHZ <= IPMC_LED_RATE_MAX_CHZ,
    "IPMC_FAULT_BLINK_CHZ must lie within IPMC_LED_RATE_MIN_CHZ..=IPMC_LED_RATE_MAX_CHZ"
);
// The heartbeat pulse is a fraction of the period 100 000 / IPMC_LED_HEARTBEAT_CHZ ms, so the
// pulse is shorter than the period when on_ms x chz < 100 000 (u64 keeps the product exact).
const _: () = assert!(
    IPMC_LED_HEARTBEAT_CHZ >= IPMC_LED_RATE_MIN_CHZ
        && IPMC_LED_HEARTBEAT_CHZ <= IPMC_LED_RATE_MAX_CHZ,
    "IPMC_LED_HEARTBEAT_CHZ must lie within IPMC_LED_RATE_MIN_CHZ..=IPMC_LED_RATE_MAX_CHZ"
);
const _: () = assert!(
    IPMC_LED_HEARTBEAT_ON_MS >= 1
        && (IPMC_LED_HEARTBEAT_ON_MS as u64) * (IPMC_LED_HEARTBEAT_CHZ as u64) < 100_000,
    "IPMC_LED_HEARTBEAT_ON_MS must be at least 1 and shorter than the heartbeat period"
);
const _: () = assert!(
    IPMC_LED_ACTIVITY_CHZ >= IPMC_LED_RATE_MIN_CHZ
        && IPMC_LED_ACTIVITY_CHZ <= IPMC_LED_RATE_MAX_CHZ,
    "IPMC_LED_ACTIVITY_CHZ must lie within IPMC_LED_RATE_MIN_CHZ..=IPMC_LED_RATE_MAX_CHZ"
);
const _: () = assert!(
    IPMC_LED_ACTIVITY_HOLD_MS > 0,
    "IPMC_LED_ACTIVITY_HOLD_MS must be above 0"
);
const _: () = assert!(!IPMC_FW_NAME.is_empty(), "IPMC_FW_NAME must not be empty");

// ---- clock ----

// HSE bypass input range (DS10086 external clock source characteristics: 1 to 50 MHz).
const _: () = assert!(
    IPMC_HSE_HZ >= 1_000_000 && IPMC_HSE_HZ <= 50_000_000,
    "IPMC_HSE_HZ must be 1000000..=50000000 (HSE bypass input)"
);
// ipmc-nucleo-f401re src/bsp/clock.rs programs 2 flash wait states (valid for 60 MHz < HCLK <= 84 MHz at
// 2.7 to 3.6 V, RM0368 3.4) and divides APB1 by 2 (APB1 is at most 42 MHz).
const _: () = assert!(
    IPMC_SYSCLK_HZ > 60_000_000 && IPMC_SYSCLK_HZ <= 84_000_000,
    "IPMC_SYSCLK_HZ must be above 60000000 and at most 84000000 (2 flash wait states, STM32F401 limit)"
);

// ---- INA226 example driver (datasheet Tables 7-3 and 7-4: three-bit fields) ----

const _: () = assert!(
    matches!(IPMC_INA226_AVG_CODE, 0..=7) && matches!(IPMC_INA226_CT_CODE, 0..=7),
    "IPMC_INA226_AVG_CODE and IPMC_INA226_CT_CODE must be 0..=7"
);

// ---- Device ID (IPMI v2.0 Table 20-2) ----

const _: () = assert!(
    matches!(IPMC_DEVICE_REVISION, 0..=15),
    "IPMC_DEVICE_REVISION must be 0..=15 (byte 3 bits 3:0)"
);
const _: () = assert!(
    IPMC_IPMI_VERSION == 0x02,
    "IPMC_IPMI_VERSION must be 0x02 (IPMI 2.0, BCD with the least significant digit in bits 7:4)"
);
const MANUFACTURER_ID_RESERVED: u32 = 0x0F_FFFF;
const PRODUCT_ID_RESERVED: u16 = 0xFFFF;
const _: () = assert!(
    IPMC_MANUFACTURER_ID >> 20 == 0 && IPMC_MANUFACTURER_ID != MANUFACTURER_ID_RESERVED,
    "IPMC_MANUFACTURER_ID must fit in 20 bits and not be 0FFFFFh (reserved)"
);
const _: () = assert!(
    IPMC_PRODUCT_ID != PRODUCT_ID_RESERVED,
    "IPMC_PRODUCT_ID must not be FFFFh (reserved)"
);

// ---- FRU strings ----

/// Longest FRU string: the type/length byte of an 8-bit ASCII field holds the length in its
/// low 6 bits (IPMI Platform Management FRU Information Storage Definition 1.0, section 13).
const FRU_STRING_MAX: usize = 63;
/// printable_ascii -- every byte of `text` is 20h..=7Eh (a define string is printable ASCII, no
/// control byte, no byte above 7Eh; the FRU type/length byte and the SDR ID string carry it as
/// 8-bit ASCII).
const fn printable_ascii(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] < 0x20 || bytes[i] > 0x7E {
            return false;
        }
        i += 1;
    }
    true
}

/// fru_string_guard -- expands to the compile-time checks that the named string define is
/// printable ASCII and fits a FRU field: `fru_string_guard!(X)` becomes
/// `const _: () = assert!(printable_ascii(X), ..);` and
/// `const _: () = assert!(X.len() <= FRU_STRING_MAX, "X is longer than 63 bytes ...");`.
macro_rules! fru_string_guard {
    ($name:ident) => {
        const _: () = assert!(
            printable_ascii($name),
            concat!(stringify!($name), " must be printable ASCII (20h..=7Eh)")
        );
        const _: () = assert!(
            $name.len() <= FRU_STRING_MAX,
            concat!(
                stringify!($name),
                " is longer than 63 bytes (FRU 8-bit ASCII type/length)"
            )
        );
    };
}
fru_string_guard!(IPMC_FRU_BOARD_MFR);
fru_string_guard!(IPMC_FRU_BOARD_PRODUCT);
fru_string_guard!(IPMC_FRU_BOARD_SERIAL);
fru_string_guard!(IPMC_FRU_BOARD_PART);
fru_string_guard!(IPMC_FRU_FILE_ID);
fru_string_guard!(IPMC_FRU_PRODUCT_MFR);
fru_string_guard!(IPMC_FRU_PRODUCT_NAME);
fru_string_guard!(IPMC_FRU_PRODUCT_PART);
fru_string_guard!(IPMC_FRU_PRODUCT_VERSION);
fru_string_guard!(IPMC_FRU_PRODUCT_SERIAL);
fru_string_guard!(IPMC_FRU_ASSET_TAG);
fru_string_guard!(IPMC_FRU_PRODUCT_FILE_ID);

fru_string_guard!(IPMC_FRU_MFG_DATE);
// IPMC_FRU_MFG_DATE is a date in `YYYY-MM-DD HH:MM` form; ipmc-fru date.rs checks the shape and
// the range at compile time with the one date parser.
// The FRU area is addressed by a 16-bit offset (IPMI v2.0 Table 34-3) and its areas are
// multiples of 8 bytes (FRU spec section 8: offsets in multiples of 8 bytes). That it holds the
// built image is checked in ipmc-fru records.rs, which owns the image length.
const _: () = assert!(
    IPMC_FRU_SIZE.is_multiple_of(8) && IPMC_FRU_SIZE >= 8 && IPMC_FRU_SIZE <= 0xFFFF,
    "IPMC_FRU_SIZE must be a multiple of 8, 8..=65535"
);
// Reserved room for the per-unit FRU fields (ipmc-fru records.rs builds each area with this
// many bytes for the field, whatever the default's length). A serial number holds at least the
// text form of a MAC address (owner 10-25-2026); a default string must fit its own room.
/// MAC_TEXT_LEN -- `00:11:22:33:44:55`, six hex pairs and five colons.
const MAC_TEXT_LEN: usize = 17;
/// fru_room_guard -- `fru_room_guard!(MAX_DEFINE, DEFAULT_DEFINE, min)` checks the room define
/// is `min..=63` and holds the default string; a failure names the define.
macro_rules! fru_room_guard {
    ($max:ident, $default:ident, $min:expr) => {
        const _: () = assert!(
            $max >= $min && $max <= FRU_STRING_MAX,
            concat!(stringify!($max), " is out of range (see defs.rs)")
        );
        const _: () = assert!(
            $default.len() <= $max,
            concat!(stringify!($default), " is longer than ", stringify!($max))
        );
    };
}
fru_room_guard!(
    IPMC_FRU_BOARD_SERIAL_MAX,
    IPMC_FRU_BOARD_SERIAL,
    MAC_TEXT_LEN
);
fru_room_guard!(IPMC_FRU_BOARD_PART_MAX, IPMC_FRU_BOARD_PART, 2);
fru_room_guard!(IPMC_FRU_PRODUCT_PART_MAX, IPMC_FRU_PRODUCT_PART, 2);
fru_room_guard!(
    IPMC_FRU_PRODUCT_SERIAL_MAX,
    IPMC_FRU_PRODUCT_SERIAL,
    MAC_TEXT_LEN
);
fru_room_guard!(IPMC_FRU_ASSET_TAG_MAX, IPMC_FRU_ASSET_TAG, 2);
fru_room_guard!(IPMC_FRU_BOARD_MFR_MAX, IPMC_FRU_BOARD_MFR, 2);
fru_room_guard!(IPMC_FRU_BOARD_PRODUCT_MAX, IPMC_FRU_BOARD_PRODUCT, 2);
fru_room_guard!(IPMC_FRU_BOARD_FILE_ID_MAX, IPMC_FRU_FILE_ID, 2);
fru_room_guard!(IPMC_FRU_PRODUCT_MFR_MAX, IPMC_FRU_PRODUCT_MFR, 2);
fru_room_guard!(IPMC_FRU_PRODUCT_NAME_MAX, IPMC_FRU_PRODUCT_NAME, 2);
fru_room_guard!(IPMC_FRU_PRODUCT_VERSION_MAX, IPMC_FRU_PRODUCT_VERSION, 2);
fru_room_guard!(IPMC_FRU_PRODUCT_FILE_ID_MAX, IPMC_FRU_PRODUCT_FILE_ID, 2);
// The flash persistence switch is a boolean; the save delay is long enough to coalesce a chunked
// Write FRU Data and short enough that a write is on flash before a person resets the board.
const _: () = assert!(
    IPMC_FRU_PERSIST_DELAY_MS >= 100 && IPMC_FRU_PERSIST_DELAY_MS <= 10_000,
    "IPMC_FRU_PERSIST_DELAY_MS must be 100..=10000"
);
// The blank-check buffer is read in whole flash words and holds at least a few of them; the upper
// bound keeps it a small stack buffer (the record is IPMC_FRU_SIZE + 16 bytes).
const _: () = assert!(
    IPMC_FRU_SCAN_CHUNK.is_multiple_of(4)
        && IPMC_FRU_SCAN_CHUNK >= 16
        && IPMC_FRU_SCAN_CHUNK <= 272,
    "IPMC_FRU_SCAN_CHUNK must be a multiple of 4, 16..=272"
);
// The quiet time before a run-time erase is longer than one IPMB frame exchange and short enough
// that a save is not held back for long on a busy bus.
const _: () = assert!(
    IPMC_FRU_ERASE_QUIET_MS >= 100 && IPMC_FRU_ERASE_QUIET_MS <= 5_000,
    "IPMC_FRU_ERASE_QUIET_MS must be 100..=5000"
);
// A frame in progress ends within T1 = 20 ms (VITA 46.11 Table 4.1.1-1); the wait covers one.
const _: () = assert!(
    IPMC_FRU_PAUSE_WAIT_MS >= 20 && IPMC_FRU_PAUSE_WAIT_MS <= 200,
    "IPMC_FRU_PAUSE_WAIT_MS must be 20..=200"
);
// The urgent save waits longer than the T5 response deadline of 227 ms (VITA 46.11 Table 4.1.1-1).
const _: () = assert!(
    IPMC_URGENT_LINK_WAIT_MS >= 228 && IPMC_URGENT_LINK_WAIT_MS <= 1_000,
    "IPMC_URGENT_LINK_WAIT_MS must be 228..=1000"
);
// A pending Cold Reset runs after at most IPMC_RESET_WAIT_MS: the urgent save's link wait, its
// pause wait and one sector erase (500 ms at most, 16 KB sector, DS10086 Table 45) fit inside it
// with the response and the save to spare, or the reset would cut the save short.
const _: () = assert!(
    IPMC_RESET_WAIT_MS > IPMC_URGENT_LINK_WAIT_MS + IPMC_FRU_PAUSE_WAIT_MS + 500,
    "IPMC_RESET_WAIT_MS must exceed IPMC_URGENT_LINK_WAIT_MS + IPMC_FRU_PAUSE_WAIT_MS + 500 (the DS10086 Table 45 sector erase maximum, 16 KB sector)"
);

// ---- SDR ID strings ----

/// SDR_NAME_MIN -- an 8-bit ASCII type/length field with data carries at least 2 characters
/// (IPMI v2.0 section 43.15). SDR_NAME_MAX -- the ID string is 16 bytes at most (Tables 43-2, 43-8).
const SDR_NAME_MIN: usize = 2;
const SDR_NAME_MAX: usize = 16;
/// sdr_name_guard -- one compile-time check that the named define is a legal SDR ID string.
macro_rules! sdr_name_guard {
    ($name:ident) => {
        const _: () = assert!(
            printable_ascii($name),
            concat!(stringify!($name), " must be printable ASCII (20h..=7Eh)")
        );
        const _: () = assert!(
            $name.len() >= SDR_NAME_MIN && $name.len() <= SDR_NAME_MAX,
            concat!(
                stringify!($name),
                " must be 2..=16 bytes (IPMI v2.0 Table 43-2, section 43.15)"
            )
        );
    };
}
sdr_name_guard!(IPMC_SDR_FRU_STATE_NAME);
sdr_name_guard!(IPMC_SDR_IPMB_LINK_NAME);
sdr_name_guard!(IPMC_SDR_FRU_HEALTH_NAME);
sdr_name_guard!(IPMC_SDR_FRU_VOLTAGE_NAME);
sdr_name_guard!(IPMC_SDR_FRU_TEMP_NAME);
sdr_name_guard!(IPMC_SDR_PAYLOAD_RESULTS_NAME);
sdr_name_guard!(IPMC_SDR_PAYLOAD_STATUS_NAME);
sdr_name_guard!(IPMC_SDR_PAYLOAD_MODE_NAME);
sdr_name_guard!(IPMC_SDR_IPMC_NAME);

// ---- sensors and board I/O ----

// Longest period: one minute. Shortest: one sample blocks the main loop for about 2 ms of ADC
// conversions (11 channel reads x IPMC_ADC_AVERAGE 8 x 23.4 us), so 10 ms keeps the loop
// mostly free.
const _: () = assert!(
    IPMC_SENSOR_PERIOD_MS >= 10 && IPMC_SENSOR_PERIOD_MS <= 60_000,
    "IPMC_SENSOR_PERIOD_MS must be 10..=60000"
);
// At most 16 averages: 11 channel reads x 16 x 23.4 us = 4.1 ms, under the 10 ms shortest
// period; the 12-bit sum of 16 conversions fits u32 with room to spare.
const _: () = assert!(
    IPMC_ADC_AVERAGE >= 1 && IPMC_ADC_AVERAGE <= 16,
    "IPMC_ADC_AVERAGE must be 1..=16"
);
// The power state machine runs once per sample, so a shorter timeout could not be honoured.
const _: () = assert!(
    IPMC_PGOOD_TIMEOUT_MS >= IPMC_SENSOR_PERIOD_MS,
    "IPMC_PGOOD_TIMEOUT_MS must be at least IPMC_SENSOR_PERIOD_MS"
);
// Rails 0..=4 are the board table in ipmc-sensors board_sensors.rs.
const BOARD_RAIL_COUNT: usize = 5;
const _: () = assert!(
    IPMC_RAILS_MAX >= BOARD_RAIL_COUNT && IPMC_RAILS_MAX <= 32,
    "IPMC_RAILS_MAX must be 5..=32 (the board table has 5 rails)"
);
const _: () = assert!(
    IPMC_RAIL_NONCRIT_PCT >= 1
        && IPMC_RAIL_NONCRIT_PCT < IPMC_RAIL_CRIT_PCT
        && IPMC_RAIL_CRIT_PCT < 100,
    "rail windows must satisfy 1 <= IPMC_RAIL_NONCRIT_PCT < IPMC_RAIL_CRIT_PCT < 100"
);
const _: () = assert!(
    !IPMC_RAIL0_NAME.is_empty() && IPMC_RAIL0_NOMINAL_MV > 0,
    "rail 0 needs a name and a nominal voltage"
);

/// external_rail_ok -- true when a divided rail is well formed: the divider terms are nonzero
/// and the pin voltage at the top of the critical window stays within the ADC reference
/// (VDDA nominal, IPMC_RAIL0_NOMINAL_MV): nominal x DEN x (100 + critical) / (NUM x 100) <= VDDA.
const fn external_rail_ok(nominal_mv: u32, num: u32, den: u32) -> bool {
    if nominal_mv == 0 || num == 0 || den == 0 {
        return false;
    }
    // u128: the product of three u32 values needs up to 96 bits.
    let pin_top_x100 = nominal_mv as u128 * den as u128 * (100 + IPMC_RAIL_CRIT_PCT as u128);
    pin_top_x100 <= IPMC_RAIL0_NOMINAL_MV as u128 * num as u128 * 100
}
/// rail_guard -- expands to the name and divider checks of one external rail:
/// `rail_guard!(NAME, NOMINAL, NUM, DEN)` becomes one `const _: () = assert!(..)`.
macro_rules! rail_guard {
    ($name:ident, $nominal:ident, $num:ident, $den:ident) => {
        const _: () = assert!(
            !$name.is_empty() && external_rail_ok($nominal, $num, $den),
            concat!(
                stringify!($name),
                ": rail needs a name, nonzero divider terms and a pin voltage within VDDA"
            )
        );
    };
}
rail_guard!(
    IPMC_RAIL1_NAME,
    IPMC_RAIL1_NOMINAL_MV,
    IPMC_RAIL1_DIV_NUM,
    IPMC_RAIL1_DIV_DEN
);
rail_guard!(
    IPMC_RAIL2_NAME,
    IPMC_RAIL2_NOMINAL_MV,
    IPMC_RAIL2_DIV_NUM,
    IPMC_RAIL2_DIV_DEN
);
rail_guard!(
    IPMC_RAIL3_NAME,
    IPMC_RAIL3_NOMINAL_MV,
    IPMC_RAIL3_DIV_NUM,
    IPMC_RAIL3_DIV_DEN
);
rail_guard!(
    IPMC_RAIL4_NAME,
    IPMC_RAIL4_NOMINAL_MV,
    IPMC_RAIL4_DIV_NUM,
    IPMC_RAIL4_DIV_DEN
);

// The die sensor works from -40 C to +125 C (DS10086 Table 14 junction range).
const _: () = assert!(
    IPMC_TEMP_LC_C >= -40 && IPMC_TEMP_UNR_C <= 125,
    "temperature thresholds must lie within the die sensor range -40..=125 C"
);
const _: () = assert!(
    IPMC_TEMP_LC_C < IPMC_TEMP_LNC_C
        && IPMC_TEMP_LNC_C < IPMC_TEMP_UNC_C
        && IPMC_TEMP_UNC_C < IPMC_TEMP_UC_C
        && IPMC_TEMP_UC_C < IPMC_TEMP_UNR_C,
    "temperature thresholds must satisfy LC < LNC < UNC < UC < UNR"
);
const _: () = assert!(
    IPMC_SIM_TEMP_C >= -40 && IPMC_SIM_TEMP_C <= IPMC_TEMP_UNR_C,
    "IPMC_SIM_TEMP_C is out of range"
);
const _: () = assert!(
    !IPMC_INPUT_PGOOD_NAME.is_empty()
        && !IPMC_INPUT_FAULT_NAME.is_empty()
        && !IPMC_INPUT_OVERTEMP_NAME.is_empty()
        && !IPMC_INPUT_RESET_NAME.is_empty(),
    "input names must not be empty"
);

// x86 emulator tunables. The delay is a transmission time inside the VITA 46.11 T5 response limit
// (227 ms, ipmc-core link.rs RESPONSE_DEADLINE_MS); a larger value would make every replayed
// response late. A vector line is a time field, a bus tag and IPMB_MSG_MAX hex tokens of three
// characters ("hh "); the allowance covers the time and the tag, and a line max below the bound
// would reject the longest legal frame.
const _: () = assert!(
    IPMC_EMU_TX_DONE_DELAY_MS >= 1 && IPMC_EMU_TX_DONE_DELAY_MS <= 227,
    "IPMC_EMU_TX_DONE_DELAY_MS must be 1..=227 (VITA 46.11 T5 response limit)"
);
/// Characters a vector line may spend on the time field, the bus tag and the separators.
const VECTOR_LINE_PREFIX_CHARS: usize = 40;
const _: () = assert!(
    IPMC_EMU_VECTOR_LINE_MAX >= VECTOR_LINE_PREFIX_CHARS + 3 * IPMB_MSG_MAX,
    "IPMC_EMU_VECTOR_LINE_MAX must hold a time, a bus tag and IPMB_MSG_MAX three-character tokens"
);
const _: () = assert!(
    IPMC_EMU_REPORT_LINES_MAX >= 1,
    "IPMC_EMU_REPORT_LINES_MAX must be at least 1"
);
// The serve-mode port is a user port: below 1024 is privileged, above 65535 is not a port.
const _: () = assert!(IPMC_EMU_PORT >= 1024, "IPMC_EMU_PORT must be 1024..=65535");
// A serve-mode response leaves at most two loop steps plus the transmission time after its
// request (the step that takes the request in, then the step at the end of the transmission);
// that must stay inside the VITA 46.11 T5 response limit (227 ms, as above).
const _: () = assert!(
    IPMC_EMU_SERVE_POLL_MS >= 1 && 2 * IPMC_EMU_SERVE_POLL_MS + IPMC_EMU_TX_DONE_DELAY_MS <= 227,
    "IPMC_EMU_SERVE_POLL_MS must be at least 1, and two of it plus IPMC_EMU_TX_DONE_DELAY_MS at most 227"
);
// The echo is a cut of one accepted line, so it can be no longer than a line.
const _: () = assert!(
    IPMC_EMU_ERROR_ECHO_MAX >= 1 && IPMC_EMU_ERROR_ECHO_MAX <= IPMC_EMU_VECTOR_LINE_MAX,
    "IPMC_EMU_ERROR_ECHO_MAX must be 1..=IPMC_EMU_VECTOR_LINE_MAX"
);

/// IPMC_BOOT_COUNTDOWN -- the boot countdown of this build, seconds: IPMC_BOOT_COUNTDOWN_FIELD
/// in the field image (`gen.ps1 build -Field`, cfg(ipmc_field)), IPMC_BOOT_COUNTDOWN_BENCH otherwise.
pub const IPMC_BOOT_COUNTDOWN: u32 = if cfg!(ipmc_field) {
    IPMC_BOOT_COUNTDOWN_FIELD
} else {
    IPMC_BOOT_COUNTDOWN_BENCH
};
/// FIELD_BUILD -- true in the field image.
pub const FIELD_BUILD: bool = cfg!(ipmc_field);

const _: () = assert!(
    IPMC_BOOT_COUNTDOWN_BENCH < 31,
    "IPMC_BOOT_COUNTDOWN_BENCH must be 0..=30"
);
const _: () = assert!(
    IPMC_BOOT_COUNTDOWN_FIELD < 31,
    "IPMC_BOOT_COUNTDOWN_FIELD must be 0..=30"
);
// The two bootloader read-back buffers are stack arrays: non-zero (a zero chunk never advances),
// and small. No alignment is relied on: the readers take byte counts.
const _: () = assert!(
    IPMC_BOOT_COMPARE_CHUNK >= 1 && IPMC_BOOT_COMPARE_CHUNK <= 255,
    "IPMC_BOOT_COMPARE_CHUNK must be 1..=255"
);
const _: () = assert!(
    IPMC_BOOT_READ_CHUNK >= 1 && IPMC_BOOT_READ_CHUNK <= 1024,
    "IPMC_BOOT_READ_CHUNK must be 1..=1024"
);
const _: () = assert!(
    IPMC_FWU_PIN_SETTLE_US >= 10 && IPMC_FWU_PIN_SETTLE_US <= 10_000,
    "IPMC_FWU_PIN_SETTLE_US must be 10..=10000"
);
const _: () = assert!(
    IPMC_FWU_LED_WAIT_CHZ >= IPMC_LED_RATE_MIN_CHZ
        && IPMC_FWU_LED_WAIT_CHZ <= IPMC_LED_RATE_MAX_CHZ,
    "IPMC_FWU_LED_WAIT_CHZ must lie within IPMC_LED_RATE_MIN_CHZ..=IPMC_LED_RATE_MAX_CHZ"
);
const _: () = assert!(
    IPMC_FWU_LOAD_IDLE_S >= 1 && IPMC_FWU_LOAD_IDLE_S <= 600,
    "IPMC_FWU_LOAD_IDLE_S must be 1..=600"
);

//! ipmc-defs - the one Rust reader of ipmi_defs.h, with the compile-time checks on its values.
//!
//! build.rs parses the header (rules in defs_parser.rs) and writes one `pub const` per define:
//! `u32` for an integer, `&str` for a string. Every crate reads a constant as
//! `ipmc_defs::NAME`; there is no second path. The guards below fail the build when a value
//! breaks a spec or datasheet rule, naming the define.
#![no_std]
#![forbid(unsafe_code)]

// The generated consts carry no rustdoc; their meaning is the comment beside each in the header.
include!(concat!(env!("OUT_DIR"), "/ipmi_defs.rs"));

// Compile-time type and {0, 1} assertions for every define (types.rs).
mod types;

/// i2c_timing -- the IPMB timing arithmetic: cycles, SCL phases and peripheral divisors.
pub mod i2c_timing;

/// TIER23_BUILT -- true when build.rs found the Tier-2/3 extension folder (or IPMC_TIER23=1
/// demanded it) and compiled its header into these constants; false in the Tier-1 build.
/// The bridge crate `ipmc-ext` selects the extension from the same detection.
pub const TIER23_BUILT: bool = cfg!(tier23);

// The guards of the Tier-2/3 defines live with that header in the extension folder.
#[cfg(tier23)]
#[rustfmt::skip] // formatted by the Tier23 gate; absent, rustfmt cannot open it
#[path = "../../../../IPMIRustFRU_Tier23/defs_guards.rs"]
mod tier23_guards;

// IPMC_TIER comes from ipmi_defs_tier23.h, or is 1 when the extension is not built.
const _: () = assert!(
    IPMC_TIER >= 1 && IPMC_TIER <= 3,
    "IPMC_TIER must be 1, 2 or 3 (VITA 46.11 section 4)"
);

// The alternatives carry their documented values: a collapsed pair (both 0) would make the `==`
// checks below select the first.
const _: () = assert!(
    IPMB_MASTER_HW == 0 && IPMB_MASTER_BITBANG == 1,
    "IPMB_MASTER_HW must be 0 and IPMB_MASTER_BITBANG 1"
);
const _: () = assert!(
    IPMB_TX_ALTERNATE == 0 && IPMB_TX_ARRIVAL == 1,
    "IPMB_TX_ALTERNATE must be 0 and IPMB_TX_ARRIVAL 1"
);
const _: () = assert!(
    IPMB_STRAP_FROM_PINS == 0 && IPMB_STRAP_FORCED == 1,
    "IPMB_STRAP_FROM_PINS must be 0 and IPMB_STRAP_FORCED 1"
);
const _: () = assert!(
    IPMB_MASTER_MODE == IPMB_MASTER_HW || IPMB_MASTER_MODE == IPMB_MASTER_BITBANG,
    "IPMB_MASTER_MODE must be IPMB_MASTER_HW or IPMB_MASTER_BITBANG"
);
const _: () = assert!(
    IPMB_TX_POLICY == IPMB_TX_ALTERNATE || IPMB_TX_POLICY == IPMB_TX_ARRIVAL,
    "IPMB_TX_POLICY must be IPMB_TX_ALTERNATE or IPMB_TX_ARRIVAL"
);
const _: () = assert!(
    IPMB_GA_SOURCE == IPMB_STRAP_FROM_PINS || IPMB_GA_SOURCE == IPMB_STRAP_FORCED,
    "IPMB_GA_SOURCE must be IPMB_STRAP_FROM_PINS or IPMB_STRAP_FORCED"
);
const _: () = assert!(
    IPMB_NVMRO_SOURCE == IPMB_STRAP_FROM_PINS || IPMB_NVMRO_SOURCE == IPMB_STRAP_FORCED,
    "IPMB_NVMRO_SOURCE must be IPMB_STRAP_FROM_PINS or IPMB_STRAP_FORCED"
);
const _: () = assert!(
    IPMB_GA_FORCED >= 1 && IPMB_GA_FORCED <= 0x1F,
    "IPMB_GA_FORCED must be 1..=0x1F (GA[4:0], 0 is invalid)"
);
const _: () = assert!(
    IPMB_NVMRO_FORCED == 0 || IPMB_NVMRO_FORCED == 1,
    "IPMB_NVMRO_FORCED must be 0 or 1"
);
const _: () = assert!(IPMB_MSG_MAX == 32, "IPMB_MSG_MAX must be 32");
const _: () = assert!(
    IPMB_BUS_A_ENABLE == 1,
    "IPMB_BUS_A_ENABLE must be 1 (VITA 46.11 Rule 4.1.1-1: IPMB-A is mandatory)"
);
const _: () = assert!(
    matches!(IPMB_BUS_B_ENABLE, 0 | 1),
    "IPMB_BUS_B_ENABLE must be 0 or 1 (IPMB-B is optional)"
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
const _: () = assert!(IPMC_IPMI_TRACE <= 1, "IPMC_IPMI_TRACE must be 0 or 1");

// The capture log ring (ipmc-core capture.rs): 1 KiB holds a few frames, 64 KiB is the most the
// 96 KiB of RAM could spare; a trace line is kept to IPMC_LOG_TEXT_MAX bytes, 32 holds a short
// status line, 200 the longest the console prints. The relations to IPMB_MSG_MAX are guarded
// beside the type (capture.rs).
const _: () = assert!(
    IPMC_LOG_SIZE >= 1024 && IPMC_LOG_SIZE <= 65536 && IPMC_LOG_SIZE.is_power_of_two(),
    "IPMC_LOG_SIZE must be a power of two, 1024..=65536 (the usable size is one byte less)"
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
// The RX ring indexes wrap with a mask (bsp/uart.rs), so its size is a power of two; one slot
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
    (IPMC_APP_UART_ENABLE == 0 || IPMC_APP_UART_ENABLE == 1)
        && (IPMC_APP_UART_FLOW == 0 || IPMC_APP_UART_FLOW == 1),
    "IPMC_APP_UART_ENABLE and IPMC_APP_UART_FLOW must be 0 or 1"
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
        && IPMC_APP_UART_TX_BUF >= 64
        && IPMC_APP_UART_TX_BUF <= 16384,
    "IPMC_APP_UART_TX_BUF must be a power of two, 64..=16384"
);
const _: () = assert!(
    IPMC_APP_LINE_MAX >= 16 && IPMC_APP_LINE_MAX + 2 < IPMC_APP_UART_RX_BUF,
    "IPMC_APP_LINE_MAX must be 16.. and, with CR LF, fit the usable IPMC_APP_UART_RX_BUF - 1"
);
const _: () = assert!(
    IPMC_APP_BYTES_PER_POLL >= 1 && IPMC_APP_BYTES_PER_POLL <= IPMC_APP_UART_RX_BUF,
    "IPMC_APP_BYTES_PER_POLL must be 1..=IPMC_APP_UART_RX_BUF"
);
const _: () = assert!(
    IPMC_APP_FRU_WRITE == 0 || IPMC_APP_FRU_WRITE == 1,
    "IPMC_APP_FRU_WRITE must be 0 or 1"
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
const _: () = assert!(
    !IPMC_FW_NAME.is_empty() && !IPMC_CONSOLE_PROMPT.is_empty(),
    "IPMC_FW_NAME and IPMC_CONSOLE_PROMPT must not be empty"
);

// ---- clock ----

// HSE bypass input range (DS10086 external clock source characteristics: 1 to 50 MHz).
const _: () = assert!(
    IPMC_HSE_HZ >= 1_000_000 && IPMC_HSE_HZ <= 50_000_000,
    "IPMC_HSE_HZ must be 1000000..=50000000 (HSE bypass input)"
);
// ipmc-nucleo-f401re bsp/clock.rs programs 2 flash wait states (valid for 60 MHz < HCLK <= 84 MHz at
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

// ---- simulated input start levels ----

const _: () = assert!(
    IPMC_SIM_PGOOD_HIGH <= 1
        && IPMC_SIM_FAULT_N_HIGH <= 1
        && IPMC_SIM_OVERTEMP_N_HIGH <= 1
        && IPMC_SIM_RESET_N_HIGH <= 1,
    "every IPMC_SIM_*_HIGH level must be 0 or 1"
);

// ---- Device ID (IPMI v2.0 Table 20-2) ----

/// fits -- true when `value` fits in `bits` bits (bits below 32).
const fn fits(value: u32, bits: u32) -> bool {
    value >> bits == 0
}

const _: () = assert!(
    fits(IPMC_DEVICE_ID, 8),
    "IPMC_DEVICE_ID must fit in one byte"
);
const _: () = assert!(
    fits(IPMC_DEVICE_REVISION, 4),
    "IPMC_DEVICE_REVISION must be 0..=15 (byte 3 bits 3:0)"
);
const _: () = assert!(
    fits(IPMC_DEVICE_PROVIDES_SDRS, 1),
    "IPMC_DEVICE_PROVIDES_SDRS must be 0 or 1"
);
const _: () = assert!(
    IPMC_IPMI_VERSION == 0x02,
    "IPMC_IPMI_VERSION must be 0x02 (IPMI 2.0, BCD with the least significant digit in bits 7:4)"
);
const _: () = assert!(
    fits(IPMC_SUPPORT_SENSOR, 1)
        && fits(IPMC_SUPPORT_SDR_REPOSITORY, 1)
        && fits(IPMC_SUPPORT_SEL, 1)
        && fits(IPMC_SUPPORT_FRU_INVENTORY, 1)
        && fits(IPMC_SUPPORT_EVENT_RECEIVER, 1)
        && fits(IPMC_SUPPORT_EVENT_GENERATOR, 1)
        && fits(IPMC_SUPPORT_BRIDGE, 1)
        && fits(IPMC_SUPPORT_CHASSIS, 1),
    "every IPMC_SUPPORT_* bit must be 0 or 1"
);
// Get Device ID bytes 8:10 hold a 20-bit manufacturer ID (bits 23:20 reserved, zero); 0FFFFFh is
// reserved (IPMI v2.0 Table 20-2). Bytes 11:12 hold the product ID; FFFFh is reserved.
const MANUFACTURER_ID_RESERVED: u32 = 0x0F_FFFF;
const PRODUCT_ID_RESERVED: u32 = 0xFFFF;
const _: () = assert!(
    fits(IPMC_MANUFACTURER_ID, 20) && IPMC_MANUFACTURER_ID != MANUFACTURER_ID_RESERVED,
    "IPMC_MANUFACTURER_ID must fit in 20 bits and not be 0FFFFFh (reserved)"
);
const _: () = assert!(
    fits(IPMC_PRODUCT_ID, 16) && IPMC_PRODUCT_ID != PRODUCT_ID_RESERVED,
    "IPMC_PRODUCT_ID must fit in 16 bits and not be FFFFh (reserved)"
);

// ---- FRU strings ----

/// Longest FRU string: the type/length byte of an 8-bit ASCII field holds the length in its
/// low 6 bits (IPMI Platform Management FRU Information Storage Definition 1.0, section 13).
const FRU_STRING_MAX: usize = 63;
/// fru_string_guard -- expands to one compile-time check that the named string define fits a
/// FRU field: `fru_string_guard!(X)` becomes
/// `const _: () = assert!(X.len() <= FRU_STRING_MAX, "X is longer than 63 bytes ...");`.
macro_rules! fru_string_guard {
    ($name:ident) => {
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

// The manufacturing date is 3 bytes, little endian (FRU spec section 11, Table 11-1).
const _: () = assert!(
    IPMC_FRU_MFG_MINUTES <= 0xFF_FFFF,
    "IPMC_FRU_MFG_MINUTES must fit 3 bytes (minutes since 1996-01-01)"
);
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
const MAC_TEXT_LEN: u32 = 17;
/// fru_room_guard -- `fru_room_guard!(MAX_DEFINE, DEFAULT_DEFINE, min)` checks the room define
/// is `min..=63` and holds the default string; a failure names the define.
macro_rules! fru_room_guard {
    ($max:ident, $default:ident, $min:expr) => {
        const _: () = assert!(
            $max >= $min && $max <= FRU_STRING_MAX as u32,
            concat!(stringify!($max), " is out of range (see ipmi_defs.h)")
        );
        const _: () = assert!(
            $default.len() as u32 <= $max,
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
// The flash persistence switch is a boolean; the save delay is long enough to coalesce a chunked
// Write FRU Data and short enough that a write is on flash before a person resets the board.
const _: () = assert!(
    IPMC_FRU_PERSIST == 0 || IPMC_FRU_PERSIST == 1,
    "IPMC_FRU_PERSIST must be 0 or 1"
);
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

// ---- SDR ID strings ----

/// SDR_NAME_MIN -- an 8-bit ASCII type/length field with data carries at least 2 characters
/// (IPMI v2.0 section 43.15). SDR_NAME_MAX -- the ID string is 16 bytes at most (Tables 43-2, 43-8).
const SDR_NAME_MIN: usize = 2;
const SDR_NAME_MAX: usize = 16;
/// sdr_name_guard -- one compile-time check that the named define is a legal SDR ID string.
macro_rules! sdr_name_guard {
    ($name:ident) => {
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

const _: () = assert!(
    IPMC_SRC_HW == 0 && IPMC_SRC_SIM == 1 && IPMC_SRC_I2C == 2 && IPMC_SRC_SPI == 3,
    "IPMC_SRC_HW, _SIM, _I2C, _SPI must be 0, 1, 2, 3"
);
/// source_valid -- true when `value` is one of the four source switch values. The firmware
/// build (ipmc-nucleo-f401re sensing.rs) accepts only HW and SIM until a bus is assigned to external parts.
const fn source_valid(value: u32) -> bool {
    value == IPMC_SRC_HW || value == IPMC_SRC_SIM || value == IPMC_SRC_I2C || value == IPMC_SRC_SPI
}
const _: () = assert!(
    source_valid(IPMC_ADC_INTERNAL_SOURCE)
        && source_valid(IPMC_ADC_EXTERNAL_SOURCE)
        && source_valid(IPMC_FAULT_SOURCE)
        && source_valid(IPMC_POWER_SOURCE),
    "every IPMC_*_SOURCE must be one of the IPMC_SRC_* values"
);
const _: () = assert!(IPMC_POWER_AT_BOOT <= 1, "IPMC_POWER_AT_BOOT must be 0 or 1");
const _: () = assert!(
    IPMC_TRACE_OFF == 0 && IPMC_TRACE_EVENTS == 1 && IPMC_TRACE_ALL == 2,
    "IPMC_TRACE_OFF, _EVENTS, _ALL must be 0, 1, 2"
);
const _: () = assert!(
    IPMC_SENSOR_TRACE <= IPMC_TRACE_ALL,
    "IPMC_SENSOR_TRACE must be IPMC_TRACE_OFF, IPMC_TRACE_EVENTS or IPMC_TRACE_ALL"
);
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
const BOARD_RAIL_COUNT: u32 = 5;
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
const VECTOR_LINE_PREFIX_CHARS: u32 = 40;
const _: () = assert!(
    IPMC_EMU_VECTOR_LINE_MAX >= VECTOR_LINE_PREFIX_CHARS + 3 * IPMB_MSG_MAX,
    "IPMC_EMU_VECTOR_LINE_MAX must hold a time, a bus tag and IPMB_MSG_MAX three-character tokens"
);
const _: () = assert!(
    IPMC_EMU_REPORT_LINES_MAX >= 1,
    "IPMC_EMU_REPORT_LINES_MAX must be at least 1"
);
// The serve-mode port is a user port: below 1024 is privileged, above 65535 is not a port.
const _: () = assert!(
    IPMC_EMU_PORT >= 1024 && IPMC_EMU_PORT <= 65535,
    "IPMC_EMU_PORT must be 1024..=65535"
);
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

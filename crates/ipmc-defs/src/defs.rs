//! defs.rs - the single build configuration of the IPMC.
//!
//! This file is plain Rust: one `pub const NAME: T = value;` per tunable, in the order of the
//! sections below. A person changes a build by editing a value here (and the small selector enums
//! `MasterMode`, `TxPolicy`, `StrapSource`, `Source` and `TraceLevel`, whose variants name the
//! alternatives of a value). The file holds no `use`, no function, no macro and no check.
//!
//! `src/lib.rs` checks every value at compile time: a type mismatch or a value outside the range
//! of the rule written beside it stops the build at the define it names. Every crate reads a value
//! as `ipmc_defs::NAME`; the bench tools read it with `tools/defs.py`.
//! Prefixes: IPMB_ for bus values, IPMC_ for controller and application values.
//! The Tier-2/3 tunables live in the private extension's `defs_tier23.rs`, which `lib.rs`
//! mounts when that folder is present.

// ==========================================================================================
// BENCH / TEST SWITCHES -- check these before a system build
// ==========================================================================================
// IPMC_EVENTS_AT_BOOT (the Tier-2/3 event generator; no effect in a Tier-1 build)
//   false = BENCH DEFAULT: events off from start-up. The IPMC sends itself Set Event Receiver FFh,
//       so nothing is retried toward a Chassis Manager that never acknowledges (the Aardvark
//       bench). The console prints the line Events: off from start-up at every boot.
//   true  = SYSTEM BUILD: event messages go to the Event Receiver 20h from power-up (VITA 46.11
//       Rule 7-5).
// IPMC_LOG_AT_BOOT: true = the capture log records from start-up; DEBUG LOG ON|OFF at run time.
// IPMC_I2C_TRACE_AT_BOOT: true = one console line per IPMB frame in and out from start-up;
//       DEBUG TRACE I2C ON|OFF at run time.
// ==========================================================================================
/// false = bench: events off from start-up (default); true = system build: events to Event Receiver
/// 20h from power-up (VITA 46.11 Rule 7-5)
pub const IPMC_EVENTS_AT_BOOT: bool = false;
/// true = the capture log records from start-up; DEBUG LOG ON|OFF at run time
pub const IPMC_LOG_AT_BOOT: bool = true;
/// true = one console line per IPMB frame in and out from start-up; DEBUG TRACE I2C ON|OFF at run
/// time
pub const IPMC_I2C_TRACE_AT_BOOT: bool = false;

/// IPMB master method.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MasterMode {
    /// hardware I2C master
    Hardware,
    /// peripheral off, pins driven open-drain, peripheral back on
    BitBang,
}

/// master method; both buses use the same one
pub const IPMB_MASTER_MODE: MasterMode = MasterMode::BitBang;

/// true = IPMB-A built in; must be true (VITA Rule 4.1.1-1: IPMB-A is mandatory)
pub const IPMB_BUS_A_ENABLE: bool = true;
/// false = IPMB-B not fitted (Tier-1 bench: one Aardvark on IPMB-A); true = IPMB-B built in (VITA
/// Rule 4.1.1-1: "should")
pub const IPMB_BUS_B_ENABLE: bool = false;
/// IPMB clock, 100 kHz
pub const IPMB_I2C_HZ: u32 = 100_000;
/// IPMB FAST-MODE clock, 400 kHz (VITA 46.11 Rule 4.1.1-4.4; Tier-3, entered with the Set IPMB
/// State speed byte); the hardware master derives its CCR from it, RM0368 18.6.8
pub const IPMB_I2C_FAST_HZ: u32 = 400_000;
/// whole IPMB frame incl. rsSA and both checksums
pub const IPMB_MSG_MAX: usize = 32;

/// Response bus policy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TxPolicy {
    /// alternate transmissions between IPMB-A and IPMB-B when both are enabled (VITA 46.11 Rule
    /// 4.1.1-6)
    Alternate,
    /// send responses on the bus the request arrived on
    Arrival,
}

/// response bus policy
pub const IPMB_TX_POLICY: TxPolicy = TxPolicy::Alternate;
/// Chassis Manager 8-bit address; spec value, the captured CM used 0x10
pub const IPMB_CM_ADDR8: u8 = 0x20;
/// responses kept for retry replay (8..=64); 64 holds about 4 s at the captured peak of 16 requests
/// per second, covering the 2.75 s requester retry window (11 x 250 ms; PLAN.md 3.3)
pub const IPMB_DUP_CACHE: usize = 64;
/// a request repeated within this time is a retry and gets the cached response; must cover the
/// requester retry window (VITA 250 ms x 11 attempts = 2750 ms) and stay below the sequence reuse
/// time of the Chassis Manager (captured live: every rqSeq reused after 3.846 s or more, so the
/// VITA 5 s expiry cannot be used)
pub const IPMB_DUP_WINDOW_MS: u32 = 3000;
/// most resends of one response frame after a failed transmit, on the alternate bus (3..=10); VITA
/// 46.11 Rule 4.1.1-7 and retry count C1 3 to 10
pub const IPMB_TX_RETRIES: u8 = 3;
/// wait for a response to a request this IPMC sent before a retry, 60..=250 ms; VITA 46.11 Rule
/// 4.1.1-9 T3max, and the retry spacing T6 (Table 4.1.1-1)
pub const IPMB_REQ_TIMEOUT_MS: u32 = 250;
/// retries of a request after its first transmission (3..=10); VITA 46.11 Rule 4.1.1-10 retries up
/// to C1max = 10: 11 transmissions x 250 ms = 2.75 s, inside the T9 sequence expiry of 5 s
pub const IPMB_REQ_RETRIES: u8 = 10;
/// least time from one new request to the next (>= 5); VITA 46.11 Table 4.1.1-1 T4
pub const IPMB_REQ_GAP_MS: u32 = 5;
/// longest wait before a pending Cold Reset is applied anyway (1..=5000): normally the processor
/// resets once the response has left the egress queue and no accepted FRU write is unsaved; the
/// flash sector erase of the FRU store takes up to 500 ms (16 KB sector, DS10086 Table 45), so the
/// wait covers the response, one erase and the write
pub const IPMC_RESET_WAIT_MS: u32 = 3000;
/// time the payload reset output PAYLOAD_RST_OUT_N (PA9, open drain, active low) is held low by a
/// Cold Reset of the payload (1..=1000); VITA 46.11 Rule 4.1.2-9
pub const IPMC_PAYLOAD_RESET_PULSE_MS: u32 = 100;
/// payload operational modes this payload supports, bit n = mode n (modes 0..=14; bit 0, the normal
/// mode, must be set; bit 15 is reserved): 3 = modes 0 and 1. The Payload Mode sensor SDR record
/// (sensor 7, Tier-3) carries it as its reading and assertion masks and Get Payload Mode
/// Capabilities answers it; VITA 46.11 Table 10.2.8-1
pub const IPMC_PAYLOAD_MODES: u16 = 3;
/// received requests the link layer holds between the bus driver and the main loop (1..=32); a full
/// FIFO drops and counts
pub const IPMB_INGRESS_DEPTH: usize = 8;
/// bit-bang half bit time, 100 kHz
pub const IPMB_BITBANG_HALF_NS: u32 = 5000;
/// bit-bang SCL low time in FAST-MODE, nanoseconds (>= 1300, I2C fast-mode tLOW; VITA 46.11 Table
/// 9.2.2-1 M0_P1b)
pub const IPMB_BITBANG_FAST_LOW_NS: u32 = 1400;
/// bit-bang SCL high time in FAST-MODE, nanoseconds (>= 600, I2C fast-mode tHIGH)
pub const IPMB_BITBANG_FAST_HIGH_NS: u32 = 1000;
/// longest wait for a stretched SCL, microseconds; VITA 46.11 Table 4.1.1-1 T8 (I2C clock low hold)
/// max 3 ms (1..=3000)
pub const IPMB_STRETCH_TIMEOUT_US: u32 = 3000;
/// longest wait for a free bus before a master write reports busy (the link then resends on the
/// other bus), milliseconds; VITA 46.11 Table 4.1.1-1 T2 min 60 ms (60..=227, the T5 response
/// limit)
pub const IPMB_BUS_FREE_TIMEOUT_MS: u32 = 60;
/// received frames each bus's interrupt can post before the main loop drains them (2..=16)
pub const IPMB_RX_QUEUE_DEPTH: usize = 4;
/// retries of a master write on the same bus after arbitration loss, before it reports failure
/// (0..=10)
pub const IPMB_ARB_RETRIES: u32 = 3;
/// response frames the egress queue holds (1..=16); a full queue drops and counts
pub const IPMB_TXQ_DEPTH: usize = 4;
/// consecutive bus-fault errors on one IPMB (stretch timeout, SCL or SDA stuck low, bus busy, slave
/// BERR or TIMEOUT) before the bus is marked faulted in the IPMB Link sensor (2..=64); address and
/// data NACKs are not bus faults; a good write or frame resets the count (VITA 46.11 Rule 9.3-10)
pub const IPMB_FAULT_STREAK: u32 = 8;
/// both lines must read idle-high continuously this long before a faulted bus is cleared
/// (100..=60000), milliseconds
pub const IPMB_FAULT_CLEAR_MS: u32 = 1000;
/// true = trace every dispatched request on the console; false = silent: the link and dispatcher
/// lines are compiled out, so they are absent from the console and from the capture log
pub const IPMC_IPMI_TRACE: bool = true;
/// capture log ring, bytes (1024..=16384, a power of two; RAM budget, the stack reservation check
/// in link.x fails a build that leaves too little; usable size IPMC_LOG_SIZE - 1 because the ring
/// keeps one slot empty): whole IPMB frames in and out and the trace lines, oldest overwritten
/// first; each DEBUG DUMP row walks the log from its start, so a larger log makes DEBUG DUMP slower
/// per row
pub const IPMC_LOG_SIZE: usize = 8192;
/// longest trace line kept in the log, bytes (32..=200); a longer line is cut and ends in ...
pub const IPMC_LOG_TEXT_MAX: usize = 160;

/// Where a strap value comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StrapSource {
    /// the strap value is read from the pins
    FromPins,
    /// the strap value comes from this file (bare board)
    Forced,
}

/// source of GA[4:0]; GAP is read or generated with it
pub const IPMB_GA_SOURCE: StrapSource = StrapSource::Forced;
/// GA[4:0] logic value: slot 1 -> HW 41h -> IPMB 82h; GAP generated for odd parity
pub const IPMB_GA_FORCED: u8 = 0x01;
/// source of the NVMRO strap
pub const IPMB_NVMRO_SOURCE: StrapSource = StrapSource::Forced;
/// false = deasserted, FRU writes allowed; true = asserted
pub const IPMB_NVMRO_FORCED: bool = false;

/// console UART baud rate, 8N1; gen.ps1 reads it (through tools/defs.py) for COM3
pub const IPMC_CONSOLE_BAUD: u32 = 115_200;
/// most console bytes one main-loop pass handles; a pass also runs at most one command line, so
/// sensor sampling and the IPMB link are serviced every iteration (1..=IPMC_UART_RX_BUF); console
/// output is interrupt driven (IPMC_UART_TX_BUF), so it does not hold the main loop for the wire
/// time
pub const IPMC_CONSOLE_BYTES_PER_POLL: usize = 16;
/// console RX ring bytes; power of two, one slot stays empty; usable bytes (size - 1) must cover
/// IPMC_MENU_LINE_MAX; the rest absorbs bytes that arrive while the main loop is busy (127 bytes =
/// 11 ms at 115200 baud)
pub const IPMC_UART_RX_BUF: usize = 256;
/// console TX ring, bytes; power of two; interrupt-driven, so console output does not hold the main
/// loop until the ring is full (4096 bytes = 356 ms at 115200 baud)
pub const IPMC_UART_TX_BUF: usize = 4096;
/// true = the application port APP_UART (USART1: TX PB6, RX PB7, CTS PA11, RTS PA12) runs; false =
/// USART1 off, its pins parked
pub const IPMC_APP_UART_ENABLE: bool = true;
/// application port baud rate, 8N1
pub const IPMC_APP_UART_BAUD: u32 = 115_200;
/// true = RTS/CTS hardware handshake on PA12/PA11; false = TX/RX only
pub const IPMC_APP_UART_FLOW: bool = false;
/// application port RX ring, bytes; power of two; at least one application line (IPMC_APP_LINE_MAX
/// + 2)
pub const IPMC_APP_UART_RX_BUF: usize = 256;
/// application port TX ring, bytes; power of two, 512..=16384; an answer longer than the ring
/// leaves in pieces (the firmware copies what fits on each main-loop pass)
pub const IPMC_APP_UART_TX_BUF: usize = 1024;
/// longest application port request line, bytes (16..=IPMC_APP_UART_RX_BUF - 3);
/// IPMC_APP_UART_RX_BUF holds at least one
pub const IPMC_APP_LINE_MAX: usize = 128;
/// most application port bytes one main-loop pass takes (1..=IPMC_APP_UART_RX_BUF)
pub const IPMC_APP_BYTES_PER_POLL: usize = 32;
/// application port prompt, printed after the banner and after every answer; the payload CPU skips
/// every line that starts with it (1..=16 printable ASCII bytes, the last one a space)
pub const IPMC_APP_PROMPT: &str = "app> ";
/// true = the console FRU menu sets every FRU field (SET) and restores the built image (RESTORE);
/// false = the console FRU menu is read only (SHOW, DUMP, STORE, FIELDS, GET) and the SET and
/// RESTORE commands do not exist
pub const IPMC_FRU_CONSOLE_WRITE: bool = true;
/// true = the application port may FRU SET the per-unit FRU fields and run IPMI Write FRU Data; false =
/// FRU SET and IPMI Write FRU Data (NetFn 0Ah, command 12h) answer ERROR WRITE_DISABLED and do not
/// run, FRU GET and the other IPMI requests (FRU Control, Set IPMB State, Set Event Receiver) still run, as
/// from the Chassis Manager
pub const IPMC_APP_FRU_WRITE: bool = true;
/// longest accepted console line in bytes; a longer line is rejected
pub const IPMC_MENU_LINE_MAX: usize = 128;
/// most whitespace-separated tokens in one console line or one command syntax
pub const IPMC_MENU_TOKENS_MAX: usize = 40;
/// bytes on one line of the console FRU DUMP (1..=16; 16 x 3 characters plus a 6-character offset
/// fit an 80-column console)
pub const IPMC_FRU_DUMP_BYTES_PER_LINE: usize = 16;
/// firmware name in the banner and SYS VERSION
pub const IPMC_FW_NAME: &str = "ipmc-nucleo-f401re";
/// root prompt text of the console menu
pub const IPMC_CONSOLE_PROMPT: &str = "ipmc";
/// lowest LED blink rate, centihertz (0.01 Hz); 0 would be off
pub const IPMC_LED_RATE_MIN_CHZ: u32 = 1;
/// highest LED blink rate, centihertz (50 Hz); the 1 ms tick toggles twice per flash
pub const IPMC_LED_RATE_MAX_CHZ: u32 = 5000;
/// LED rate that signals a start-up failure the console cannot report, centihertz (5 Hz)
pub const IPMC_FAULT_BLINK_CHZ: u32 = 500;
/// LED heartbeat rate, centihertz (1 Hz): one pulse per period
pub const IPMC_LED_HEARTBEAT_CHZ: u32 = 100;
/// LED heartbeat pulse width, milliseconds on at the start of each period; must be shorter than the
/// period
pub const IPMC_LED_HEARTBEAT_ON_MS: u32 = 100;
/// LED blink rate while IPMB frames are moving, centihertz (10 Hz, 50 % duty)
pub const IPMC_LED_ACTIVITY_CHZ: u32 = 1000;
/// LED stays in the activity rate this long after the last IPMB frame, milliseconds; must be above
/// 0
pub const IPMC_LED_ACTIVITY_HOLD_MS: u32 = 200;

/// HSE input, Hz: the ST-LINK MCO clock into OSC_IN (UM1724 6.7.1), used in bypass; the PLL setting
/// in ipmc-nucleo-f401re bsp/clock.rs must produce IPMC_SYSCLK_HZ from it (compile-time check)
pub const IPMC_HSE_HZ: u32 = 8_000_000;
/// system clock, Hz; the PLL M, N, P, Q values in ipmc-nucleo-f401re bsp/clock.rs produce it from
/// IPMC_HSE_HZ and from the 16 MHz HSI (compile-time check)
pub const IPMC_SYSCLK_HZ: u32 = 84_000_000;

/// Get Device ID byte 2: device ID (IPMI v2.0 Table 20-2); 00h means unspecified
pub const IPMC_DEVICE_ID: u8 = 0x01;
/// Get Device ID byte 3 bits 3:0: device revision, 0..15
pub const IPMC_DEVICE_REVISION: u8 = 0;
/// Get Device ID byte 3 bit 7: true = the device provides Device SDRs
pub const IPMC_DEVICE_PROVIDES_SDRS: bool = true;
/// Get Device ID byte 6: IPMI version, BCD with the least significant digit in bits 7:4; 02h = IPMI
/// 2.0
pub const IPMC_IPMI_VERSION: u8 = 0x02;
/// Get Device ID byte 7 bit 0: Sensor Device
pub const IPMC_SUPPORT_SENSOR: bool = true;
/// byte 7 bit 1: SDR Repository Device
pub const IPMC_SUPPORT_SDR_REPOSITORY: bool = false;
/// byte 7 bit 2: SEL Device
pub const IPMC_SUPPORT_SEL: bool = false;
/// byte 7 bit 3: FRU Inventory Device
pub const IPMC_SUPPORT_FRU_INVENTORY: bool = true;
/// byte 7 bit 4: IPMB Event Receiver
pub const IPMC_SUPPORT_EVENT_RECEIVER: bool = false;
/// byte 7 bit 5: IPMB Event Generator
pub const IPMC_SUPPORT_EVENT_GENERATOR: bool = false;
/// byte 7 bit 6: Bridge
pub const IPMC_SUPPORT_BRIDGE: bool = false;
/// byte 7 bit 7: Chassis Device
pub const IPMC_SUPPORT_CHASSIS: bool = false;
/// Get Device ID bytes 8:10: IANA enterprise number, 20 bits (0FFFFFh is reserved); 000000h =
/// unspecified until a number is registered, another party's number is never borrowed
pub const IPMC_MANUFACTURER_ID: u32 = 0x000000;
/// Get Device ID bytes 11:12: product ID, 16 bits (FFFFh is reserved) ("IK")
pub const IPMC_PRODUCT_ID: u16 = 0x494B;
/// Get Device ID bytes 13:16: auxiliary firmware revision, 32 bits, least significant byte first
pub const IPMC_AUX_FIRMWARE: u32 = 0;

/// FRU board area: manufacturer; each FRU string is at most 63 bytes (8-bit ASCII type/length)
pub const IPMC_FRU_BOARD_MFR: &str = "IronKernel";
/// FRU board area: product name
pub const IPMC_FRU_BOARD_PRODUCT: &str = "VPX Tier-1 IPMC";
/// FRU board area: serial number
pub const IPMC_FRU_BOARD_SERIAL: &str = "IK-T1-000001";
/// FRU board area: part number
pub const IPMC_FRU_BOARD_PART: &str = "IK-IPMC-T1-NUC";
/// FRU board area: FRU file ID (the board file ID; the product area has its own,
/// IPMC_FRU_PRODUCT_FILE_ID)
pub const IPMC_FRU_FILE_ID: &str = "ipmc-fru-v1";
/// FRU product area: manufacturer
pub const IPMC_FRU_PRODUCT_MFR: &str = "IronKernel";
/// FRU product area: product name
pub const IPMC_FRU_PRODUCT_NAME: &str = "Tier-1 IPMC Reference";
/// FRU product area: part or model number
pub const IPMC_FRU_PRODUCT_PART: &str = "IK-IPMC-T1";
/// FRU product area: version; the default must equal the major.minor of the crate version (guard in
/// ipmc-core identity.rs); a value written later with SET is not checked against it
pub const IPMC_FRU_PRODUCT_VERSION: &str = "0.1";
/// FRU product area: serial number
pub const IPMC_FRU_PRODUCT_SERIAL: &str = "IK-T1-000001";
/// FRU product area: asset tag, empty
pub const IPMC_FRU_ASSET_TAG: &str = "";
/// FRU product area: FRU file ID, empty (the built product area carries the no-data field)
pub const IPMC_FRU_PRODUCT_FILE_ID: &str = "";
/// room reserved for the board manufacturer, bytes (2..=63); every FRU field has reserved room (the
/// pooled pad at the end of each area is the sum over the area's fields of MAX less the default's
/// length), so SET rewrites a field in place and no area moves
pub const IPMC_FRU_BOARD_MFR_MAX: usize = 24;
/// room reserved for the board product name, bytes (2..=63)
pub const IPMC_FRU_BOARD_PRODUCT_MAX: usize = 32;
/// room reserved for the board FRU file ID, bytes (2..=63)
pub const IPMC_FRU_BOARD_FILE_ID_MAX: usize = 16;
/// room reserved for the product manufacturer, bytes (2..=63)
pub const IPMC_FRU_PRODUCT_MFR_MAX: usize = 24;
/// room reserved for the product name, bytes (2..=63)
pub const IPMC_FRU_PRODUCT_NAME_MAX: usize = 32;
/// room reserved for the product version, bytes (2..=63)
pub const IPMC_FRU_PRODUCT_VERSION_MAX: usize = 16;
/// room reserved for the product FRU file ID, bytes (2..=63)
pub const IPMC_FRU_PRODUCT_FILE_ID_MAX: usize = 16;
/// room reserved for the board serial number, bytes (17..=63); the built board area holds this many
/// bytes for it whatever the default's length, so an update up to this length rewrites it in place;
/// at least 17, the text form of a MAC address 00:11:22:33:44:55
pub const IPMC_FRU_BOARD_SERIAL_MAX: usize = 24;
/// room reserved for the board part number, bytes (2..=63)
pub const IPMC_FRU_BOARD_PART_MAX: usize = 24;
/// room reserved for the product part or model number, bytes (2..=63)
pub const IPMC_FRU_PRODUCT_PART_MAX: usize = 24;
/// room reserved for the product serial number, bytes (17..=63); at least 17 for a MAC address text
pub const IPMC_FRU_PRODUCT_SERIAL_MAX: usize = 24;
/// room reserved for the asset tag, bytes (2..=63)
pub const IPMC_FRU_ASSET_TAG_MAX: usize = 24;
/// FRU manufacturing date, YYYY-MM-DD HH:MM (UTC), 1996-01-01 00:01 to 9999-12-31 23:59 (checked at
/// compile time by ipmc-fru date.rs); the built image holds it in the VITA 46.11 Alternate
/// Manufacturing Date/Time record (VITA 34h, a 32-bit minutes count, MultiRecord area) and leaves
/// the standard 3-byte Mfg Date/Time field 000000h (unspecified), because that field ends at
/// 2027-11-24 20:15
pub const IPMC_FRU_MFG_DATE: &str = "2026-10-23 00:00";
/// FRU inventory area size in bytes (Get FRU Inventory Area Info): the built image with every
/// field's reserved room (12 text fields) and the 14-byte Alternate Mfg Date/Time record; the flash
/// record is 528 bytes, 31 records per 16 KB sector; a multiple of 8, at least the built image
/// (compile-time guard in ipmc-fru records.rs), at most 65535; the persisted record (ipmc-fru
/// persist.rs) is 16 bytes longer than this
pub const IPMC_FRU_SIZE: usize = 512;
/// true = Write FRU Data survives reset (FRU image kept in the flash store, sector 1 alone, 16 KB);
/// false = RAM only, a reset restores the built image
pub const IPMC_FRU_PERSIST: bool = true;
/// quiet time after the last Write FRU Data before the image is saved to flash, milliseconds
/// (100..=10000); coalesces a Chassis Manager writing an area in chunks into one record
pub const IPMC_FRU_PERSIST_DELAY_MS: u32 = 1000;
/// bytes read at a time when the flash log checks a range for erased flash (ipmc-fru persist.rs); a
/// multiple of 4, 16..=272; the buffer lives on the stack
pub const IPMC_FRU_SCAN_CHUNK: usize = 64;
/// a run-time sector erase starts only after the egress queue is empty and no IPMB frame arrived
/// for this many milliseconds (100..=5000); the save stays pending in RAM meanwhile
pub const IPMC_FRU_ERASE_QUIET_MS: u32 = 500;
/// longest wait for an IPMB slave to go idle before a run-time erase, milliseconds (20..=200); a
/// frame in progress ends within T1 = 20 ms (VITA 46.11 Table 4.1.1-1)
pub const IPMC_FRU_PAUSE_WAIT_MS: u32 = 40;
/// longest wait of the urgent save before a Cold Reset for the IPMB link to hold no accepted
/// request and no untransmitted response, milliseconds (228..=1000); longer than the 227 ms
/// response deadline (T5, VITA 46.11 Table 4.1.1-1), so a request accepted just before is answered
/// first; after it the erase runs anyway because the reset is imminent
pub const IPMC_URGENT_LINK_WAIT_MS: u32 = 250;

/// sensor 0: ID string of its SDR, 2..=16 bytes (IPMI v2.0 section 43.15: 8-bit ASCII needs at
/// least 2 characters; Table 43-2 allows 16)
pub const IPMC_SDR_FRU_STATE_NAME: &str = "FRU State";
/// sensor 1: ID string of its SDR, 2..=16 bytes (IPMI v2.0 section 43.15: 8-bit ASCII needs at
/// least 2 characters; Table 43-2 allows 16)
pub const IPMC_SDR_IPMB_LINK_NAME: &str = "IPMB Link";
/// sensor 2: ID string of its SDR, 2..=16 bytes (IPMI v2.0 section 43.15: 8-bit ASCII needs at
/// least 2 characters; Table 43-2 allows 16)
pub const IPMC_SDR_FRU_HEALTH_NAME: &str = "FRU Health";
/// sensor 3: ID string of its SDR, 2..=16 bytes (IPMI v2.0 section 43.15: 8-bit ASCII needs at
/// least 2 characters; Table 43-2 allows 16)
pub const IPMC_SDR_FRU_VOLTAGE_NAME: &str = "FRU Voltage";
/// sensor 4: ID string of its SDR, 2..=16 bytes (IPMI v2.0 section 43.15: 8-bit ASCII needs at
/// least 2 characters; Table 43-2 allows 16)
pub const IPMC_SDR_FRU_TEMP_NAME: &str = "FRU Temperature";
/// sensor 5: ID string of its SDR, 2..=16 bytes (IPMI v2.0 section 43.15: 8-bit ASCII needs at
/// least 2 characters; Table 43-2 allows 16)
pub const IPMC_SDR_PAYLOAD_RESULTS_NAME: &str = "Payload Results";
/// sensor 6: ID string of its SDR, 2..=16 bytes (IPMI v2.0 section 43.15: 8-bit ASCII needs at
/// least 2 characters; Table 43-2 allows 16)
pub const IPMC_SDR_PAYLOAD_STATUS_NAME: &str = "Payload Status";
/// sensor 7 (Tier-3 only): ID string of its SDR, 2..=16 bytes (IPMI v2.0 section 43.15: 8-bit ASCII
/// needs at least 2 characters; Table 43-2 allows 16)
pub const IPMC_SDR_PAYLOAD_MODE_NAME: &str = "Payload Mode";
/// management controller device locator record (SDR type 12h) ID string, 2..=16 bytes; at most 6 so
/// the whole record fits one Get Device SDR response (IPMI v2.0 Table 43-8: 16 + N bytes)
pub const IPMC_SDR_IPMC_NAME: &str = "IPMC";

/// Source switch of a sensor or board input.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// real hardware
    Hw,
    /// simulation, adjusted from the SIM console group
    Sim,
    /// external I2C part (drivers/i2c); documented only, the firmware build refuses it until a bus
    /// is assigned
    I2c,
    /// external SPI part (drivers/spi); documented only, the firmware build refuses it until a bus
    /// is assigned
    Spi,
}

/// VDDA (from VREFINT) and die temperature: ADC1 channels 17 and 18
pub const IPMC_ADC_INTERNAL_SOURCE: Source = Source::Hw;
/// rails on A0..A3 (PA0, PA1, PA4, PB0); unwired on the bench, so simulated: each rail starts at
/// its nominal mV
pub const IPMC_ADC_EXTERNAL_SOURCE: Source = Source::Sim;
/// PGOOD, FAULT_N, OVERTEMP_N, PAYLOAD_RESET_N pins; pull-ups make an unwired pin read no-fault, a
/// jumper to GND asserts
pub const IPMC_FAULT_SOURCE: Source = Source::Hw;
/// PWR_EN output: HW drives PB5, SIM only traces the request
pub const IPMC_POWER_SOURCE: Source = Source::Hw;
/// true = request payload power at start-up; false = stay off until POWER ON
pub const IPMC_POWER_AT_BOOT: bool = true;
/// longest wait for PGOOD after PWR_EN is asserted; at least IPMC_SENSOR_PERIOD_MS, the power state
/// machine runs once per sample
pub const IPMC_PGOOD_TIMEOUT_MS: u32 = 1000;
/// time between sensor samples, milliseconds (10..=60000); one sample blocks the main loop for
/// about 2 ms of ADC conversions
pub const IPMC_SENSOR_PERIOD_MS: u32 = 1000;
/// ADC1 conversions averaged per channel read (1..=16); each takes 23.4 us
pub const IPMC_ADC_AVERAGE: u32 = 8;
/// INA226 example driver: AVG field of the Configuration register, 0..7 = 1, 4, 16, 64, 128, 256,
/// 512, 1024 averages (datasheet Table 7-3)
pub const IPMC_INA226_AVG_CODE: u8 = 0;
/// INA226 example driver: VBUSCT and VSHCT fields, 0..7 = 140 us, 204 us, 332 us, 588 us, 1.1 ms,
/// 2.116 ms, 4.156 ms, 8.244 ms (datasheet Table 7-4)
pub const IPMC_INA226_CT_CODE: u8 = 4;

/// Sensor trace level.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TraceLevel {
    /// no console lines
    Off,
    /// one line per change (rail status, temperature flags, input assert/clear, power transition,
    /// Tier-1 summary)
    Events,
    /// the change lines plus one sample line every IPMC_SENSOR_PERIOD_MS
    All,
}

/// sensor trace level at start-up; SENSORS TRACE {OFF|EVENTS|ALL} sets it at run time
pub const IPMC_SENSOR_TRACE: TraceLevel = TraceLevel::Events;
/// capacity of the sensor snapshot rail array; the board rail table must fit
pub const IPMC_RAILS_MAX: usize = 8;
/// rail non-critical window, +/- percent of nominal
pub const IPMC_RAIL_NONCRIT_PCT: u32 = 5;
/// rail critical window, +/- percent of nominal; above the non-critical window
pub const IPMC_RAIL_CRIT_PCT: u32 = 10;
/// rail 0: ADC supply, measured from VREFINT (internal, no divider)
pub const IPMC_RAIL0_NAME: &str = "VDDA";
/// rail 0 nominal, millivolts
pub const IPMC_RAIL0_NOMINAL_MV: u32 = 3300;
/// rail 1: A0, PA0, ADC1 channel 0
pub const IPMC_RAIL1_NAME: &str = "12V payload";
/// rail 1 nominal, millivolts
pub const IPMC_RAIL1_NOMINAL_MV: u32 = 12_000;
/// rail 1 divider: rail mV = pin mV x NUM / DEN (12 V -> 3 V at the pin)
pub const IPMC_RAIL1_DIV_NUM: u32 = 4;
/// rail 1 divider denominator
pub const IPMC_RAIL1_DIV_DEN: u32 = 1;
/// rail 2: A1, PA1, ADC1 channel 1
pub const IPMC_RAIL2_NAME: &str = "5V";
/// rail 2 nominal, millivolts
pub const IPMC_RAIL2_NOMINAL_MV: u32 = 5000;
/// rail 2 divider numerator (5 V -> 2.5 V at the pin)
pub const IPMC_RAIL2_DIV_NUM: u32 = 2;
/// rail 2 divider denominator
pub const IPMC_RAIL2_DIV_DEN: u32 = 1;
/// rail 3: A2, PA4, ADC1 channel 4
pub const IPMC_RAIL3_NAME: &str = "3V3 payload";
/// rail 3 nominal, millivolts
pub const IPMC_RAIL3_NOMINAL_MV: u32 = 3300;
/// rail 3 divider numerator (3.3 V -> 1.65 V at the pin, headroom for the +10 % window)
pub const IPMC_RAIL3_DIV_NUM: u32 = 2;
/// rail 3 divider denominator
pub const IPMC_RAIL3_DIV_DEN: u32 = 1;
/// rail 4: A3, PB0, ADC1 channel 8
pub const IPMC_RAIL4_NAME: &str = "+3.3V_AUX";
/// rail 4 nominal, millivolts
pub const IPMC_RAIL4_NOMINAL_MV: u32 = 3300;
/// rail 4 divider numerator (3.3 V -> 1.65 V at the pin, headroom for the +10 % window)
pub const IPMC_RAIL4_DIV_NUM: u32 = 2;
/// rail 4 divider denominator
pub const IPMC_RAIL4_DIV_DEN: u32 = 1;
/// lower non-critical threshold, degrees C: reading <= it sets the flag
pub const IPMC_TEMP_LNC_C: i32 = 0;
/// lower critical threshold, degrees C
pub const IPMC_TEMP_LC_C: i32 = -10;
/// upper non-critical threshold, degrees C: reading >= it sets the flag
pub const IPMC_TEMP_UNC_C: i32 = 70;
/// upper critical threshold, degrees C
pub const IPMC_TEMP_UC_C: i32 = 85;
/// upper non-recoverable threshold, degrees C
pub const IPMC_TEMP_UNR_C: i32 = 100;
/// simulated die temperature at start-up, degrees C
pub const IPMC_SIM_TEMP_C: i32 = 25;
/// simulated PGOOD pin level at start-up: true = high (power good), false = low; the emulator also
/// makes PGOOD follow PWR_EN when true
pub const IPMC_SIM_PGOOD_HIGH: bool = true;
/// simulated FAULT_N pin level at start-up: true = high (no fault), false = low (asserted)
pub const IPMC_SIM_FAULT_N_HIGH: bool = true;
/// simulated OVERTEMP_N pin level at start-up: true = high (no fault), false = low (asserted)
pub const IPMC_SIM_OVERTEMP_N_HIGH: bool = true;
/// simulated PAYLOAD_RESET_N pin level at start-up: true = high (not in reset), false = low
/// (asserted)
pub const IPMC_SIM_RESET_N_HIGH: bool = true;
/// input name: supplies good (PB10), active high
pub const IPMC_INPUT_PGOOD_NAME: &str = "PGOOD";
/// input name: supply fault (PA10), active low
pub const IPMC_INPUT_FAULT_NAME: &str = "FAULT_N";
/// input name: board over-temperature (PC7), active low
pub const IPMC_INPUT_OVERTEMP_NAME: &str = "OVERTEMP_N";
/// input name: payload held in reset (PA6), active low
pub const IPMC_INPUT_RESET_NAME: &str = "PAYLOAD_RESET_N";

/// x86 emulator: time from a request to the end of its response transmission, milliseconds
/// (1..=227, the VITA 46.11 T5 response limit)
pub const IPMC_EMU_TX_DONE_DELAY_MS: u32 = 1;
/// x86 emulator: longest accepted vector file line, characters; must hold a time, a bus tag and
/// IPMB_MSG_MAX three-character hex tokens
pub const IPMC_EMU_VECTOR_LINE_MAX: usize = 512;
/// x86 emulator: most vector line numbers the replay report lists for malformed frames
pub const IPMC_EMU_REPORT_LINES_MAX: usize = 8;
/// x86 emulator serve mode: default TCP port on 127.0.0.1 (1024..=65535)
pub const IPMC_EMU_PORT: u16 = 5470;
/// x86 emulator serve mode: main-loop step period and socket read timeout, milliseconds; two steps
/// plus IPMC_EMU_TX_DONE_DELAY_MS stay within the VITA 46.11 T5 response limit (227 ms)
pub const IPMC_EMU_SERVE_POLL_MS: u32 = 1;
/// x86 emulator serve mode: most characters of a rejected client line echoed in its #error reply
/// (1..=IPMC_EMU_VECTOR_LINE_MAX)
pub const IPMC_EMU_ERROR_ECHO_MAX: usize = 80;

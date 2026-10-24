/* ipmi_defs.h - the single build configuration of the IPMC.
 *
 * The Rust build parses this file (crates/ipmc-defs/build.rs); the bench tools read the values
 * they share with the IPMC (IPMC_EMU_PORT, and IPMC_CONSOLE_BAUD in gen.ps1). The file stays
 * valid C.
 * Only "#define NAME <integer, previously defined NAME or string>" lines, the include guard,
 * blank lines and C block comments are allowed. An integer is 0x hex or decimal without a
 * leading 0 (C reads that as octal) and becomes a Rust u32. A signed integer carries the tag
 * [i32] as the first token of its trailing block comment (#define NAME -10, then a comment
 * that starts with [i32]): a decimal with an optional leading minus, no leading 0, "-0"
 * refused, within i32; it becomes a Rust i32 and refers only to other [i32] defines. A
 * negative value without the tag, an unknown or malformed tag, or a mixed reference fails
 * the build. A string is one literal of printable ASCII (0x20..0x7E)
 * with no backslash, no embedded quote and no adjacent literal; "" is legal. A line ending in
 * a backslash, or anything else, fails the Rust build with its line number.
 * Prefixes: IPMB_ for bus values, IPMC_ for controller and application values.
 */
#ifndef IPMI_DEFS_H
#define IPMI_DEFS_H

#define IPMB_MASTER_HW 0                          /* hardware I2C master */
#define IPMB_MASTER_BITBANG 1                     /* peripheral off, pins driven open-drain, peripheral back on */
#define IPMB_MASTER_MODE IPMB_MASTER_BITBANG      /* master method; both buses use the same one */

#define IPMB_BUS_A_ENABLE 1                       /* 1 = IPMB-A built in; must be 1 (VITA Rule 4.1.1-1: IPMB-A is mandatory) */
#define IPMB_BUS_B_ENABLE 0                       /* 0 = IPMB-B not fitted (Tier-1 bench: one Aardvark on IPMB-A); 1 = IPMB-B built in (VITA Rule 4.1.1-1: "should") */
#define IPMB_I2C_HZ 100000                        /* IPMB clock, 100 kHz */
#define IPMB_MSG_MAX 32                           /* whole IPMB frame incl. rsSA and both checksums */
#define IPMB_TX_ALTERNATE 0                       /* alternate transmissions between IPMB-A and IPMB-B when both are enabled (VITA 46.11 Rule 4.1.1-6) */
#define IPMB_TX_ARRIVAL 1                         /* send responses on the bus the request arrived on */
#define IPMB_TX_POLICY IPMB_TX_ALTERNATE          /* response bus policy */
#define IPMB_CM_ADDR8 0x20                        /* Chassis Manager 8-bit address; spec value, the captured CM used 0x10 */
#define IPMB_DUP_CACHE 64                         /* responses kept for retry replay (8..=64); 64 holds about 4 s at the captured peak of 16 requests per second, covering the 2.75 s requester retry window (11 x 250 ms; PLAN.md 3.3) */
#define IPMB_DUP_WINDOW_MS 3000                   /* a request repeated within this time is a retry and gets the cached response; must cover the requester retry window (VITA 250 ms x 11 attempts = 2750 ms) and stay below the sequence reuse time of the Chassis Manager (captured live: every rqSeq reused after 3.846 s or more, so the VITA 5 s expiry cannot be used) */
#define IPMB_TX_RETRIES 3                         /* most resends of one response frame after a failed transmit, on the alternate bus (3..=10); VITA 46.11 Rule 4.1.1-7 and retry count C1 3 to 10 */
#define IPMB_REQ_TIMEOUT_MS 250                   /* wait for a response to a request this IPMC sent before a retry, 60..=250 ms; VITA 46.11 Rule 4.1.1-9 T3max, and the retry spacing T6 (Table 4.1.1-1) */
#define IPMB_REQ_RETRIES 10                       /* retries of a request after its first transmission (3..=10); VITA 46.11 Rule 4.1.1-10 retries up to C1max = 10: 11 transmissions x 250 ms = 2.75 s, inside the T9 sequence expiry of 5 s */
#define IPMB_REQ_GAP_MS 5                         /* least time from one new request to the next (>= 5); VITA 46.11 Table 4.1.1-1 T4 */
#define IPMC_RESET_WAIT_MS 3000                   /* longest wait before a pending Cold Reset is applied anyway (1..=5000): normally the processor resets once the response has left the egress queue and no accepted FRU write is unsaved; a flash sector erase takes up to 2 s (DS10086 Table 45), so the wait covers the response, one erase and the write */
#define IPMB_INGRESS_DEPTH 8                      /* received requests the link layer holds between the bus driver and the main loop (1..=32); a full FIFO drops and counts */
#define IPMB_BITBANG_HALF_NS 5000                 /* bit-bang half bit time, 100 kHz */
#define IPMB_STRETCH_TIMEOUT_US 3000              /* longest wait for a stretched SCL, microseconds; VITA 46.11 Table 4.1.1-1 T8 (I2C clock low hold) max 3 ms (1..=3000) */
#define IPMB_BUS_FREE_TIMEOUT_MS 60               /* longest wait for a free bus before a master write reports busy (the link then resends on the other bus), milliseconds; VITA 46.11 Table 4.1.1-1 T2 min 60 ms (60..=227, the T5 response limit) */
#define IPMB_RX_QUEUE_DEPTH 4                     /* received frames each bus's interrupt can post before the main loop drains them (2..=16) */
#define IPMB_ARB_RETRIES 3                        /* retries of a master write on the same bus after arbitration loss, before it reports failure (0..=10) */
#define IPMB_TXQ_DEPTH 4                          /* response frames the egress queue holds (1..16); a full queue drops and counts */
#define IPMB_FAULT_STREAK 8                       /* consecutive bus-fault errors on one IPMB (stretch timeout, SCL or SDA stuck low, bus busy, slave BERR or TIMEOUT) before the bus is marked faulted in the IPMB Link sensor (2..=64); address and data NACKs are not bus faults; a good write or frame resets the count (VITA 46.11 Rule 9.3-10) */
#define IPMB_FAULT_CLEAR_MS 1000                  /* both lines must read idle-high continuously this long before a faulted bus is cleared (100..=60000), milliseconds */
#define IPMC_IPMI_TRACE 1                         /* 1 = trace every dispatched request on the console; 0 = silent */

#define IPMB_STRAP_FROM_PINS 0                    /* strap value is read from the pins */
#define IPMB_STRAP_FORCED 1                       /* strap value comes from this file (bare board) */
#define IPMB_GA_SOURCE IPMB_STRAP_FORCED          /* source of GA[4:0]; GAP is read or generated with it */
#define IPMB_GA_FORCED 0x01                       /* GA[4:0] logic value: slot 1 -> HW 41h -> IPMB 82h; GAP generated for odd parity */
#define IPMB_NVMRO_SOURCE IPMB_STRAP_FORCED       /* source of the NVMRO strap */
#define IPMB_NVMRO_FORCED 0                       /* 0 = deasserted, FRU writes allowed; 1 = asserted */

#define IPMC_CONSOLE_BAUD 115200                  /* console UART baud rate, 8N1; gen.ps1 reads this line for COM3 */
#define IPMC_CONSOLE_BYTES_PER_POLL 16            /* most console bytes one main-loop pass handles; a pass also runs at most one command line, so sensor sampling and the IPMB link are serviced every iteration (1..IPMC_UART_RX_BUF); blocking console output is a known cost the Phase 6 link scheduler accounts for (227 ms response deadline, VITA Rule 4.1.1-11) */
#define IPMC_UART_RX_BUF 128                      /* console RX ring bytes; power of two, one slot stays empty; usable bytes (size - 1) must cover IPMC_MENU_LINE_MAX; the rest (47 bytes = 4.1 ms at 115200 baud) absorbs bytes that arrive during a blocking transmit */
#define IPMC_MENU_LINE_MAX 80                     /* longest accepted console line in bytes; a longer line is rejected */
#define IPMC_MENU_TOKENS_MAX 8                    /* most whitespace-separated tokens in one console line or one command syntax */
#define IPMC_FRU_DUMP_BYTES_PER_LINE 16           /* bytes on one line of the console FRU DUMP (1..16; 16 x 3 characters plus a 6-character offset fit an 80-column console) */
#define IPMC_FW_NAME "ipmc-nucleo-f401re"                    /* firmware name in the banner and SYS VERSION */
#define IPMC_CONSOLE_PROMPT "ipmc"                /* root prompt text of the console menu */
#define IPMC_LED_RATE_MIN_CHZ 1                   /* lowest LED blink rate, centihertz (0.01 Hz); 0 would be off */
#define IPMC_LED_RATE_MAX_CHZ 5000                /* highest LED blink rate, centihertz (50 Hz); the 1 ms tick toggles twice per flash */
#define IPMC_FAULT_BLINK_CHZ 500                  /* LED rate that signals a start-up failure the console cannot report, centihertz (5 Hz) */
#define IPMC_LED_HEARTBEAT_CHZ 100                /* LED heartbeat rate, centihertz (1 Hz): one pulse per period */
#define IPMC_LED_HEARTBEAT_ON_MS 100              /* LED heartbeat pulse width, milliseconds on at the start of each period; must be shorter than the period */
#define IPMC_LED_ACTIVITY_CHZ 1000                /* LED blink rate while IPMB frames are moving, centihertz (10 Hz, 50 % duty) */
#define IPMC_LED_ACTIVITY_HOLD_MS 200             /* LED stays in the activity rate this long after the last IPMB frame, milliseconds; must be above 0 */

#define IPMC_HSE_HZ 8000000                       /* HSE input, Hz: the ST-LINK MCO clock into OSC_IN (UM1724 6.7.1), used in bypass; the PLL setting in ipmc-nucleo-f401re bsp/clock.rs must produce IPMC_SYSCLK_HZ from it (compile-time check) */
#define IPMC_SYSCLK_HZ 84000000                   /* system clock, Hz; the PLL M, N, P, Q values in ipmc-nucleo-f401re bsp/clock.rs produce it from IPMC_HSE_HZ and from the 16 MHz HSI (compile-time check) */

#define IPMC_DEVICE_ID 0x01                       /* Get Device ID byte 2: device ID (IPMI v2.0 Table 20-2); 00h means unspecified */
#define IPMC_DEVICE_REVISION 0                    /* Get Device ID byte 3 bits 3:0: device revision, 0..15 */
#define IPMC_DEVICE_PROVIDES_SDRS 1               /* Get Device ID byte 3 bit 7: 1 = the device provides Device SDRs */
#define IPMC_IPMI_VERSION 0x02                    /* Get Device ID byte 6: IPMI version, BCD with the least significant digit in bits 7:4; 02h = IPMI 2.0 */
#define IPMC_SUPPORT_SENSOR 1                     /* Get Device ID byte 7 bit 0: Sensor Device */
#define IPMC_SUPPORT_SDR_REPOSITORY 0             /* byte 7 bit 1: SDR Repository Device */
#define IPMC_SUPPORT_SEL 0                        /* byte 7 bit 2: SEL Device */
#define IPMC_SUPPORT_FRU_INVENTORY 1              /* byte 7 bit 3: FRU Inventory Device */
#define IPMC_SUPPORT_EVENT_RECEIVER 0             /* byte 7 bit 4: IPMB Event Receiver */
#define IPMC_SUPPORT_EVENT_GENERATOR 0            /* byte 7 bit 5: IPMB Event Generator */
#define IPMC_SUPPORT_BRIDGE 0                     /* byte 7 bit 6: Bridge */
#define IPMC_SUPPORT_CHASSIS 0                    /* byte 7 bit 7: Chassis Device */
#define IPMC_MANUFACTURER_ID 0x000000             /* Get Device ID bytes 8:10: IANA enterprise number, 20 bits (0FFFFFh is reserved); 000000h = unspecified until a number is registered, another party's number is never borrowed */
#define IPMC_PRODUCT_ID 0x494B                    /* Get Device ID bytes 11:12: product ID, 16 bits (FFFFh is reserved) ("IK") */
#define IPMC_AUX_FIRMWARE 0                       /* Get Device ID bytes 13:16: auxiliary firmware revision, 32 bits, least significant byte first */

#define IPMC_FRU_BOARD_MFR "IronKernel"           /* FRU board area: manufacturer; each FRU string is at most 63 bytes (8-bit ASCII type/length) */
#define IPMC_FRU_BOARD_PRODUCT "VPX Tier-1 IPMC"  /* FRU board area: product name */
#define IPMC_FRU_BOARD_SERIAL "IK-T1-000001"      /* FRU board area: serial number */
#define IPMC_FRU_BOARD_PART "IK-IPMC-T1-NUC"      /* FRU board area: part number */
#define IPMC_FRU_FILE_ID "ipmc-fru-v1"            /* FRU board area: FRU file ID */
#define IPMC_FRU_PRODUCT_MFR "IronKernel"         /* FRU product area: manufacturer */
#define IPMC_FRU_PRODUCT_NAME "Tier-1 IPMC Reference" /* FRU product area: product name */
#define IPMC_FRU_PRODUCT_PART "IK-IPMC-T1"        /* FRU product area: part or model number */
#define IPMC_FRU_PRODUCT_VERSION "0.1"            /* FRU product area: version; must equal the major.minor of the crate version (guard in ipmc-core identity.rs) */
#define IPMC_FRU_PRODUCT_SERIAL "IK-T1-000001"    /* FRU product area: serial number */
#define IPMC_FRU_ASSET_TAG ""                     /* FRU product area: asset tag, empty */
#define IPMC_FRU_MFG_MINUTES 16204320             /* FRU board area manufacturing date, minutes since 1996-01-01 00:00 (FRU spec section 11, 3 bytes little endian, 000000h = unspecified); 2026-10-23 00:00 = (30 years x 365 + 8 leap days + 295 days) = 11253 days x 1440 = 16204320; at most 16777215 */
#define IPMC_FRU_SIZE 256                         /* FRU inventory area size in bytes (Get FRU Inventory Area Info); a multiple of 8, at least the built image (compile-time guard in ipmc-fru records.rs), at most 65535; the persisted record (ipmc-fru persist.rs) is 16 bytes longer than this */
#define IPMC_FRU_PERSIST 1                        /* 1 = Write FRU Data survives reset (FRU image kept in the flash log of sectors 6 and 7); 0 = RAM only, a reset restores the built image */
#define IPMC_FRU_PERSIST_DELAY_MS 1000            /* quiet time after the last Write FRU Data before the image is saved to flash, milliseconds (100..=10000); coalesces a Chassis Manager writing an area in chunks into one record */
#define IPMC_FRU_SCAN_CHUNK 64                    /* bytes read at a time when the flash log checks a range for erased flash (ipmc-fru persist.rs); a multiple of 4, 16..=272; the buffer lives on the stack */
#define IPMC_FRU_ERASE_QUIET_MS 500               /* a run-time sector erase starts only after the egress queue is empty and no IPMB frame arrived for this many milliseconds (100..=5000); the save stays pending in RAM meanwhile */
#define IPMC_FRU_PAUSE_WAIT_MS 40                 /* longest wait for an IPMB slave to go idle before a run-time erase, milliseconds (20..=200); a frame in progress ends within T1 = 20 ms (VITA 46.11 Table 4.1.1-1) */

#define IPMC_SDR_FRU_STATE_NAME "FRU State"           /* sensor 0: ID string of its SDR, 2..=16 bytes (IPMI v2.0 section 43.15: 8-bit ASCII needs at least 2 characters; Table 43-2 allows 16) */
#define IPMC_SDR_IPMB_LINK_NAME "IPMB Link"           /* sensor 1: ID string of its SDR, 2..=16 bytes (IPMI v2.0 section 43.15: 8-bit ASCII needs at least 2 characters; Table 43-2 allows 16) */
#define IPMC_SDR_FRU_HEALTH_NAME "FRU Health"         /* sensor 2: ID string of its SDR, 2..=16 bytes (IPMI v2.0 section 43.15: 8-bit ASCII needs at least 2 characters; Table 43-2 allows 16) */
#define IPMC_SDR_FRU_VOLTAGE_NAME "FRU Voltage"       /* sensor 3: ID string of its SDR, 2..=16 bytes (IPMI v2.0 section 43.15: 8-bit ASCII needs at least 2 characters; Table 43-2 allows 16) */
#define IPMC_SDR_FRU_TEMP_NAME "FRU Temperature"      /* sensor 4: ID string of its SDR, 2..=16 bytes (IPMI v2.0 section 43.15: 8-bit ASCII needs at least 2 characters; Table 43-2 allows 16) */
#define IPMC_SDR_PAYLOAD_RESULTS_NAME "Payload Results" /* sensor 5: ID string of its SDR, 2..=16 bytes (IPMI v2.0 section 43.15: 8-bit ASCII needs at least 2 characters; Table 43-2 allows 16) */
#define IPMC_SDR_PAYLOAD_STATUS_NAME "Payload Status" /* sensor 6: ID string of its SDR, 2..=16 bytes (IPMI v2.0 section 43.15: 8-bit ASCII needs at least 2 characters; Table 43-2 allows 16) */
#define IPMC_SDR_IPMC_NAME "IPMC"                     /* management controller device locator record (SDR type 12h) ID string, 2..=16 bytes; at most 6 so the whole record fits one Get Device SDR response (IPMI v2.0 Table 43-8: 16 + N bytes) */

#define IPMC_SRC_HW 0                             /* source switch value: real hardware */
#define IPMC_SRC_SIM 1                            /* source switch value: simulation, adjusted from the SIM console group */
#define IPMC_SRC_I2C 2                            /* source switch value: external I2C part (drivers/i2c); documented only, the firmware build refuses it until a bus is assigned */
#define IPMC_SRC_SPI 3                            /* source switch value: external SPI part (drivers/spi); documented only, the firmware build refuses it until a bus is assigned */
#define IPMC_ADC_INTERNAL_SOURCE IPMC_SRC_HW      /* VDDA (from VREFINT) and die temperature: ADC1 channels 17 and 18 */
#define IPMC_ADC_EXTERNAL_SOURCE IPMC_SRC_SIM     /* rails on A0..A3 (PA0, PA1, PA4, PB0); unwired on the bench, so simulated: each rail starts at its nominal mV */
#define IPMC_FAULT_SOURCE IPMC_SRC_HW             /* PGOOD, FAULT_N, OVERTEMP_N, PAYLOAD_RESET_N pins; pull-ups make an unwired pin read no-fault, a jumper to GND asserts */
#define IPMC_POWER_SOURCE IPMC_SRC_HW             /* PWR_EN output: HW drives PB5, SIM only traces the request */
#define IPMC_POWER_AT_BOOT 1                      /* 1 = request payload power at start-up; 0 = stay off until POWER ON */
#define IPMC_PGOOD_TIMEOUT_MS 1000                /* longest wait for PGOOD after PWR_EN is asserted; at least IPMC_SENSOR_PERIOD_MS, the power state machine runs once per sample */
#define IPMC_SENSOR_PERIOD_MS 1000                /* time between sensor samples, milliseconds (10..=60000); one sample blocks the main loop for about 2 ms of ADC conversions */
#define IPMC_ADC_AVERAGE 8                        /* ADC1 conversions averaged per channel read (1..=16); each takes 23.4 us */
#define IPMC_INA226_AVG_CODE 0                    /* INA226 example driver: AVG field of the Configuration register, 0..7 = 1, 4, 16, 64, 128, 256, 512, 1024 averages (datasheet Table 7-3) */
#define IPMC_INA226_CT_CODE 4                     /* INA226 example driver: VBUSCT and VSHCT fields, 0..7 = 140 us, 204 us, 332 us, 588 us, 1.1 ms, 2.116 ms, 4.156 ms, 8.244 ms (datasheet Table 7-4) */
#define IPMC_TRACE_OFF 0                          /* sensor trace level: no console lines */
#define IPMC_TRACE_EVENTS 1                       /* sensor trace level: one line per change (rail status, temperature flags, input assert/clear, power transition, Tier-1 summary) */
#define IPMC_TRACE_ALL 2                          /* sensor trace level: the change lines plus one sample line every IPMC_SENSOR_PERIOD_MS */
#define IPMC_SENSOR_TRACE IPMC_TRACE_EVENTS       /* sensor trace level at start-up; SENSORS TRACE {OFF|EVENTS|ALL} sets it at run time */
#define IPMC_RAILS_MAX 8                          /* capacity of the sensor snapshot rail array; the board rail table must fit */
#define IPMC_RAIL_NONCRIT_PCT 5                   /* rail non-critical window, +/- percent of nominal */
#define IPMC_RAIL_CRIT_PCT 10                     /* rail critical window, +/- percent of nominal; above the non-critical window */
#define IPMC_RAIL0_NAME "VDDA"                    /* rail 0: ADC supply, measured from VREFINT (internal, no divider) */
#define IPMC_RAIL0_NOMINAL_MV 3300                /* rail 0 nominal, millivolts */
#define IPMC_RAIL1_NAME "12V payload"             /* rail 1: A0, PA0, ADC1 channel 0 */
#define IPMC_RAIL1_NOMINAL_MV 12000               /* rail 1 nominal, millivolts */
#define IPMC_RAIL1_DIV_NUM 4                      /* rail 1 divider: rail mV = pin mV x NUM / DEN (12 V -> 3 V at the pin) */
#define IPMC_RAIL1_DIV_DEN 1                      /* rail 1 divider denominator */
#define IPMC_RAIL2_NAME "5V"                      /* rail 2: A1, PA1, ADC1 channel 1 */
#define IPMC_RAIL2_NOMINAL_MV 5000                /* rail 2 nominal, millivolts */
#define IPMC_RAIL2_DIV_NUM 2                      /* rail 2 divider numerator (5 V -> 2.5 V at the pin) */
#define IPMC_RAIL2_DIV_DEN 1                      /* rail 2 divider denominator */
#define IPMC_RAIL3_NAME "3V3 payload"             /* rail 3: A2, PA4, ADC1 channel 4 */
#define IPMC_RAIL3_NOMINAL_MV 3300                /* rail 3 nominal, millivolts */
#define IPMC_RAIL3_DIV_NUM 2                      /* rail 3 divider numerator (3.3 V -> 1.65 V at the pin, headroom for the +10 % window) */
#define IPMC_RAIL3_DIV_DEN 1                      /* rail 3 divider denominator */
#define IPMC_RAIL4_NAME "1V0 core"                /* rail 4: A3, PB0, ADC1 channel 8 */
#define IPMC_RAIL4_NOMINAL_MV 1000                /* rail 4 nominal, millivolts */
#define IPMC_RAIL4_DIV_NUM 1                      /* rail 4 divider numerator (direct) */
#define IPMC_RAIL4_DIV_DEN 1                      /* rail 4 divider denominator */
#define IPMC_TEMP_LNC_C 0                         /* [i32] lower non-critical threshold, degrees C: reading <= it sets the flag */
#define IPMC_TEMP_LC_C -10                        /* [i32] lower critical threshold, degrees C */
#define IPMC_TEMP_UNC_C 70                        /* [i32] upper non-critical threshold, degrees C: reading >= it sets the flag */
#define IPMC_TEMP_UC_C 85                         /* [i32] upper critical threshold, degrees C */
#define IPMC_TEMP_UNR_C 100                       /* [i32] upper non-recoverable threshold, degrees C */
#define IPMC_SIM_TEMP_C 25                        /* [i32] simulated die temperature at start-up, degrees C */
#define IPMC_SIM_PGOOD_HIGH 1                     /* simulated PGOOD pin level at start-up: 1 = high (power good), 0 = low */
#define IPMC_SIM_FAULT_N_HIGH 1                   /* simulated FAULT_N pin level at start-up: 1 = high (no fault), 0 = low (asserted) */
#define IPMC_SIM_OVERTEMP_N_HIGH 1                /* simulated OVERTEMP_N pin level at start-up: 1 = high (no fault), 0 = low (asserted) */
#define IPMC_SIM_RESET_N_HIGH 1                   /* simulated PAYLOAD_RESET_N pin level at start-up: 1 = high (not in reset), 0 = low (asserted) */
#define IPMC_INPUT_PGOOD_NAME "PGOOD"             /* input name: supplies good (PB10), active high */
#define IPMC_INPUT_FAULT_NAME "FAULT_N"           /* input name: supply fault (PA10), active low */
#define IPMC_INPUT_OVERTEMP_NAME "OVERTEMP_N"     /* input name: board over-temperature (PC7), active low */
#define IPMC_INPUT_RESET_NAME "PAYLOAD_RESET_N"   /* input name: payload held in reset (PB6), active low */

#define IPMC_EMU_TX_DONE_DELAY_MS 1               /* x86 emulator: time from a request to the end of its response transmission, milliseconds (1..=227, the VITA 46.11 T5 response limit) */
#define IPMC_EMU_VECTOR_LINE_MAX 512              /* x86 emulator: longest accepted vector file line, characters; must hold a time, a bus tag and IPMB_MSG_MAX three-character hex tokens */
#define IPMC_EMU_REPORT_LINES_MAX 8               /* x86 emulator: most vector line numbers the replay report lists for malformed frames */
#define IPMC_EMU_PORT 5470                        /* x86 emulator serve mode: default TCP port on 127.0.0.1 (1024..=65535) */
#define IPMC_EMU_SERVE_POLL_MS 1                  /* x86 emulator serve mode: main-loop step period and socket read timeout, milliseconds; two steps plus IPMC_EMU_TX_DONE_DELAY_MS stay within the VITA 46.11 T5 response limit (227 ms) */
#define IPMC_EMU_ERROR_ECHO_MAX 80                /* x86 emulator serve mode: most characters of a rejected client line echoed in its #error reply (1..=IPMC_EMU_VECTOR_LINE_MAX) */

#endif /* IPMI_DEFS_H */

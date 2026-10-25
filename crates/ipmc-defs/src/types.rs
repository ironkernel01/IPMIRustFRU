//! types.rs - compile-time TYPE and range assertions for every define the code consumes.
//!
//! The header tags a define `[i32]` in its trailing comment to make it signed; without the tag
//! it is `u32`. A wrong tag would still compile where the consumer converts with `as`, so each
//! define is pinned here to the type its consumers expect: tagging a count or a boolean `[i32]`
//! (or dropping the tag from a temperature) fails the build at this file. A boolean is also
//! pinned to the set {0, 1}: a signed `-1` cannot pass for "true".
//! Adding a define to ipmi_defs.h adds its line here (the list is the whole header).
use super::*;

// Unsigned integers (u32).
// IPMC_TIER comes from ipmi_defs_tier23.h, or is 1 when the extension is not built.
const _: u32 = IPMC_TIER;
const _: u32 = IPMB_MASTER_HW;
const _: u32 = IPMB_MASTER_BITBANG;
const _: u32 = IPMB_MASTER_MODE;
const _: u32 = IPMB_BUS_A_ENABLE;
const _: u32 = IPMB_BUS_B_ENABLE;
const _: u32 = IPMB_I2C_HZ;
const _: u32 = IPMB_I2C_FAST_HZ;
const _: u32 = IPMB_MSG_MAX;
const _: u32 = IPMB_TX_ALTERNATE;
const _: u32 = IPMB_TX_ARRIVAL;
const _: u32 = IPMB_TX_POLICY;
const _: u32 = IPMB_CM_ADDR8;
const _: u32 = IPMB_DUP_CACHE;
const _: u32 = IPMB_DUP_WINDOW_MS;
const _: u32 = IPMB_TX_RETRIES;
const _: u32 = IPMB_INGRESS_DEPTH;
const _: u32 = IPMB_BITBANG_HALF_NS;
const _: u32 = IPMB_BITBANG_FAST_LOW_NS;
const _: u32 = IPMB_BITBANG_FAST_HIGH_NS;
const _: u32 = IPMC_PAYLOAD_RESET_PULSE_MS;
const _: u32 = IPMC_PAYLOAD_MODES;
const _: u32 = IPMB_STRETCH_TIMEOUT_US;
const _: u32 = IPMB_BUS_FREE_TIMEOUT_MS;
const _: u32 = IPMB_RX_QUEUE_DEPTH;
const _: u32 = IPMB_ARB_RETRIES;
const _: u32 = IPMB_TXQ_DEPTH;
const _: u32 = IPMB_FAULT_STREAK;
const _: u32 = IPMB_FAULT_CLEAR_MS;
const _: u32 = IPMC_IPMI_TRACE;
const _: u32 = IPMC_LOG_SIZE;
const _: u32 = IPMC_LOG_AT_BOOT;
const _: u32 = IPMC_LOG_TEXT_MAX;
const _: u32 = IPMC_I2C_TRACE_AT_BOOT;
const _: u32 = IPMC_EVENTS_AT_BOOT;
const _: u32 = IPMB_STRAP_FROM_PINS;
const _: u32 = IPMB_STRAP_FORCED;
const _: u32 = IPMB_GA_SOURCE;
const _: u32 = IPMB_GA_FORCED;
const _: u32 = IPMB_NVMRO_SOURCE;
const _: u32 = IPMB_NVMRO_FORCED;
const _: u32 = IPMC_CONSOLE_BAUD;
const _: u32 = IPMC_CONSOLE_BYTES_PER_POLL;
const _: u32 = IPMC_UART_RX_BUF;
const _: u32 = IPMC_MENU_LINE_MAX;
const _: u32 = IPMC_MENU_TOKENS_MAX;
const _: u32 = IPMC_LED_RATE_MIN_CHZ;
const _: u32 = IPMC_LED_RATE_MAX_CHZ;
const _: u32 = IPMC_FAULT_BLINK_CHZ;
const _: u32 = IPMC_LED_HEARTBEAT_CHZ;
const _: u32 = IPMC_LED_HEARTBEAT_ON_MS;
const _: u32 = IPMC_LED_ACTIVITY_CHZ;
const _: u32 = IPMC_LED_ACTIVITY_HOLD_MS;
const _: u32 = IPMC_HSE_HZ;
const _: u32 = IPMC_SYSCLK_HZ;
const _: u32 = IPMC_DEVICE_ID;
const _: u32 = IPMC_DEVICE_REVISION;
const _: u32 = IPMC_DEVICE_PROVIDES_SDRS;
const _: u32 = IPMC_IPMI_VERSION;
const _: u32 = IPMC_SUPPORT_SENSOR;
const _: u32 = IPMC_SUPPORT_SDR_REPOSITORY;
const _: u32 = IPMC_SUPPORT_SEL;
const _: u32 = IPMC_SUPPORT_FRU_INVENTORY;
const _: u32 = IPMC_SUPPORT_EVENT_RECEIVER;
const _: u32 = IPMC_SUPPORT_EVENT_GENERATOR;
const _: u32 = IPMC_SUPPORT_BRIDGE;
const _: u32 = IPMC_SUPPORT_CHASSIS;
const _: u32 = IPMC_MANUFACTURER_ID;
const _: u32 = IPMC_PRODUCT_ID;
const _: u32 = IPMC_AUX_FIRMWARE;
const _: u32 = IPMC_FRU_MFG_MINUTES;
const _: u32 = IPMC_FRU_SIZE;
const _: u32 = IPMC_FRU_PERSIST;
const _: u32 = IPMC_FRU_PERSIST_DELAY_MS;
const _: u32 = IPMC_FRU_SCAN_CHUNK;
const _: u32 = IPMC_FRU_ERASE_QUIET_MS;
const _: u32 = IPMC_FRU_PAUSE_WAIT_MS;
const _: u32 = IPMC_SRC_HW;
const _: u32 = IPMC_SRC_SIM;
const _: u32 = IPMC_SRC_I2C;
const _: u32 = IPMC_SRC_SPI;
const _: u32 = IPMC_ADC_INTERNAL_SOURCE;
const _: u32 = IPMC_ADC_EXTERNAL_SOURCE;
const _: u32 = IPMC_FAULT_SOURCE;
const _: u32 = IPMC_POWER_SOURCE;
const _: u32 = IPMC_POWER_AT_BOOT;
const _: u32 = IPMC_PGOOD_TIMEOUT_MS;
const _: u32 = IPMC_SENSOR_PERIOD_MS;
const _: u32 = IPMC_ADC_AVERAGE;
const _: u32 = IPMC_INA226_AVG_CODE;
const _: u32 = IPMC_INA226_CT_CODE;
const _: u32 = IPMC_TRACE_OFF;
const _: u32 = IPMC_TRACE_EVENTS;
const _: u32 = IPMC_TRACE_ALL;
const _: u32 = IPMC_SENSOR_TRACE;
const _: u32 = IPMC_RAILS_MAX;
const _: u32 = IPMC_RAIL_NONCRIT_PCT;
const _: u32 = IPMC_RAIL_CRIT_PCT;
const _: u32 = IPMC_RAIL0_NOMINAL_MV;
const _: u32 = IPMC_RAIL1_NOMINAL_MV;
const _: u32 = IPMC_RAIL1_DIV_NUM;
const _: u32 = IPMC_RAIL1_DIV_DEN;
const _: u32 = IPMC_RAIL2_NOMINAL_MV;
const _: u32 = IPMC_RAIL2_DIV_NUM;
const _: u32 = IPMC_RAIL2_DIV_DEN;
const _: u32 = IPMC_RAIL3_NOMINAL_MV;
const _: u32 = IPMC_RAIL3_DIV_NUM;
const _: u32 = IPMC_RAIL3_DIV_DEN;
const _: u32 = IPMC_RAIL4_NOMINAL_MV;
const _: u32 = IPMC_RAIL4_DIV_NUM;
const _: u32 = IPMC_RAIL4_DIV_DEN;
const _: u32 = IPMC_SIM_PGOOD_HIGH;
const _: u32 = IPMC_SIM_FAULT_N_HIGH;
const _: u32 = IPMC_SIM_OVERTEMP_N_HIGH;
const _: u32 = IPMC_SIM_RESET_N_HIGH;
const _: u32 = IPMC_EMU_TX_DONE_DELAY_MS;
const _: u32 = IPMC_EMU_VECTOR_LINE_MAX;
const _: u32 = IPMC_EMU_REPORT_LINES_MAX;
const _: u32 = IPMC_EMU_PORT;
const _: u32 = IPMC_EMU_SERVE_POLL_MS;
const _: u32 = IPMC_EMU_ERROR_ECHO_MAX;

// Signed integers (i32): the temperatures, tagged `[i32]` in the header.
const _: i32 = IPMC_TEMP_LNC_C;
const _: i32 = IPMC_TEMP_LC_C;
const _: i32 = IPMC_TEMP_UNC_C;
const _: i32 = IPMC_TEMP_UC_C;
const _: i32 = IPMC_TEMP_UNR_C;
const _: i32 = IPMC_SIM_TEMP_C;

// Strings.
const _: &str = IPMC_FW_NAME;
const _: &str = IPMC_CONSOLE_PROMPT;
const _: &str = IPMC_FRU_BOARD_MFR;
const _: &str = IPMC_FRU_BOARD_PRODUCT;
const _: &str = IPMC_FRU_BOARD_SERIAL;
const _: &str = IPMC_FRU_BOARD_PART;
const _: &str = IPMC_FRU_FILE_ID;
const _: &str = IPMC_FRU_PRODUCT_MFR;
const _: &str = IPMC_FRU_PRODUCT_NAME;
const _: &str = IPMC_FRU_PRODUCT_PART;
const _: &str = IPMC_FRU_PRODUCT_VERSION;
const _: &str = IPMC_FRU_PRODUCT_SERIAL;
const _: &str = IPMC_FRU_ASSET_TAG;
const _: &str = IPMC_SDR_FRU_STATE_NAME;
const _: &str = IPMC_SDR_IPMB_LINK_NAME;
const _: &str = IPMC_SDR_FRU_HEALTH_NAME;
const _: &str = IPMC_SDR_FRU_VOLTAGE_NAME;
const _: &str = IPMC_SDR_FRU_TEMP_NAME;
const _: &str = IPMC_SDR_PAYLOAD_RESULTS_NAME;
const _: &str = IPMC_SDR_PAYLOAD_STATUS_NAME;
const _: &str = IPMC_SDR_PAYLOAD_MODE_NAME;
const _: &str = IPMC_SDR_IPMC_NAME;
const _: &str = IPMC_RAIL0_NAME;
const _: &str = IPMC_RAIL1_NAME;
const _: &str = IPMC_RAIL2_NAME;
const _: &str = IPMC_RAIL3_NAME;
const _: &str = IPMC_RAIL4_NAME;
const _: &str = IPMC_INPUT_PGOOD_NAME;
const _: &str = IPMC_INPUT_FAULT_NAME;
const _: &str = IPMC_INPUT_OVERTEMP_NAME;
const _: &str = IPMC_INPUT_RESET_NAME;

// Booleans: exactly 0 or 1.
const _: () = assert!(
    matches!(IPMB_BUS_A_ENABLE, 0 | 1),
    "IPMB_BUS_A_ENABLE must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMB_BUS_B_ENABLE, 0 | 1),
    "IPMB_BUS_B_ENABLE must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_IPMI_TRACE, 0 | 1),
    "IPMC_IPMI_TRACE must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_LOG_AT_BOOT, 0 | 1),
    "IPMC_LOG_AT_BOOT must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_I2C_TRACE_AT_BOOT, 0 | 1),
    "IPMC_I2C_TRACE_AT_BOOT must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_EVENTS_AT_BOOT, 0 | 1),
    "IPMC_EVENTS_AT_BOOT must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMB_NVMRO_FORCED, 0 | 1),
    "IPMB_NVMRO_FORCED must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_DEVICE_PROVIDES_SDRS, 0 | 1),
    "IPMC_DEVICE_PROVIDES_SDRS must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_SUPPORT_SENSOR, 0 | 1),
    "IPMC_SUPPORT_SENSOR must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_SUPPORT_SDR_REPOSITORY, 0 | 1),
    "IPMC_SUPPORT_SDR_REPOSITORY must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_SUPPORT_SEL, 0 | 1),
    "IPMC_SUPPORT_SEL must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_SUPPORT_FRU_INVENTORY, 0 | 1),
    "IPMC_SUPPORT_FRU_INVENTORY must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_SUPPORT_EVENT_RECEIVER, 0 | 1),
    "IPMC_SUPPORT_EVENT_RECEIVER must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_SUPPORT_EVENT_GENERATOR, 0 | 1),
    "IPMC_SUPPORT_EVENT_GENERATOR must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_SUPPORT_BRIDGE, 0 | 1),
    "IPMC_SUPPORT_BRIDGE must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_SUPPORT_CHASSIS, 0 | 1),
    "IPMC_SUPPORT_CHASSIS must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_POWER_AT_BOOT, 0 | 1),
    "IPMC_POWER_AT_BOOT must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_SIM_PGOOD_HIGH, 0 | 1),
    "IPMC_SIM_PGOOD_HIGH must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_SIM_FAULT_N_HIGH, 0 | 1),
    "IPMC_SIM_FAULT_N_HIGH must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_SIM_OVERTEMP_N_HIGH, 0 | 1),
    "IPMC_SIM_OVERTEMP_N_HIGH must be 0 or 1"
);
const _: () = assert!(
    matches!(IPMC_SIM_RESET_N_HIGH, 0 | 1),
    "IPMC_SIM_RESET_N_HIGH must be 0 or 1"
);

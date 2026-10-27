//! flash_map - `crates/ipmc-nucleo-f401re/flash_map.x` (the linker's one place of every address)
//! against `ipmc_boot::geometry` (the same numbers as Rust constants). A difference fails the
//! test: a bootloader and a runtime built from drifted copies would disagree on the header or
//! the request address (Documentation/ipmc_boot_plan.md section 2).

use ipmc_boot::geometry::{
    APP_END, APP_START, BOOT_END, BOOT_START, NOINIT_END, RAM_END, RAM_START, STORE_END,
    STORE_START,
};
use ipmc_boot::header::{HEADER_ADDR, HEADER_OFFSET};

const FLASH_MAP: &str = include_str!("../../ipmc-nucleo-f401re/flash_map.x");

/// number -- a linker number: decimal or 0x hex, with an optional K (1024) suffix.
fn number(text: &str) -> u32 {
    let text = text.trim();
    let (digits, scale) = match text.strip_suffix('K') {
        Some(rest) => (rest.trim(), 1024),
        None => (text, 1),
    };
    let value = match digits.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => digits.parse(),
    };
    value.unwrap_or_else(|e| panic!("flash_map.x number {text:?}: {e}")) * scale
}

/// region -- (ORIGIN, LENGTH) of the MEMORY line that starts with `name :`. LENGTH may be a
/// difference `96K - 256`.
fn region(name: &str) -> (u32, u32) {
    let line = FLASH_MAP
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with(name) && l[name.len()..].trim_start().starts_with(':'))
        .unwrap_or_else(|| panic!("flash_map.x has no region {name}"));
    let body = line.split("/*").next().unwrap_or(line);
    let origin = body
        .split("ORIGIN =")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .unwrap_or_else(|| panic!("no ORIGIN in {line:?}"));
    let length = body
        .split("LENGTH =")
        .nth(1)
        .unwrap_or_else(|| panic!("no LENGTH in {line:?}"));
    let length = match length.split_once('-') {
        Some((left, right)) => number(left) - number(right),
        None => number(length),
    };
    (number(origin), length)
}

/// symbol -- the value of `name = ORIGIN(X) + 0x...;` lines: the region origin plus the offset.
fn symbol_offset(name: &str) -> u32 {
    let line = FLASH_MAP
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with(name) && l[name.len()..].trim_start().starts_with('='))
        .unwrap_or_else(|| panic!("flash_map.x has no symbol {name}"));
    line.split('+')
        .nth(1)
        .map_or(0, |offset| number(offset.trim_end_matches(';')))
}

#[test]
fn the_regions_equal_the_geometry_constants() {
    // (region, start, end) -- end is one past the last byte.
    let rows = [
        ("BOOT", BOOT_START, BOOT_END),
        ("STORE", STORE_START, STORE_END),
        ("APP", APP_START, APP_END),
        ("NOINIT", RAM_START, NOINIT_END),
        ("RAM", NOINIT_END, RAM_END),
    ];
    for (name, start, end) in rows {
        let (origin, length) = region(name);
        assert_eq!((origin, origin + length), (start, end), "{name}");
    }
}

#[test]
fn the_header_and_request_symbols_equal_the_header_constants() {
    assert_eq!(symbol_offset("__app_header"), HEADER_OFFSET);
    assert_eq!(APP_START + symbol_offset("__app_header"), HEADER_ADDR);
    assert_eq!(symbol_offset("__boot_request"), 0);
    // The kept fault record is the last 16 bytes of NOINIT.
    assert_eq!(symbol_offset("__fault_record"), NOINIT_END - RAM_START - 16);
}

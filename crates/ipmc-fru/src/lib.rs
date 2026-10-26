//! ipmc-fru - the FRU Information records of FRU 0 and the scratchpad memory behind Read FRU
//! Data and Write FRU Data.
//!
//! The zero checksum and the byte builder live in ipmc-wire. The crate holds every byte layout of the FRU image and the RAM copy a Chassis Manager reads
//! and writes. It has no hardware access and no allocator, and builds for the host and for
//! thumbv7em-none-eabihf. File map:
//!
//! - `records.rs`: the common header, the board and product info areas, the MultiRecord area with
//!   the Alternate Mfg Date/Time record, the compile-time image built from the IPMC_FRU_* defines,
//!   the decoders and `verify`.
//! - `store.rs`: `FruStore`, the scratchpad memory, and `FruError`.
//! - `date.rs`: the manufacturing date, minutes since 1996-01-01 and their calendar text.
//! - `unit.rs`: the thirteen FRU fields (`FruField`: the text fields of both areas and the
//!   manufacturing date) and the editor that rewrites one in place inside the room
//!   `records.rs` reserves.
//! - `persist.rs`: the stored record, the one-sector flash log behind the `FlashLog` trait, the
//!   boot load and the save after a Write FRU Data.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

/// date -- the manufacturing date: conversions between minutes and calendar text.
pub mod date;
/// persist -- the FRU scratchpad on flash: record format, log, load and save.
pub mod persist;
/// records -- FRU Information layouts, the built image, the decoder and `verify`.
pub mod records;
/// store -- the FRU scratchpad memory and the FRU error type.
pub mod store;
/// unit -- the per-unit FRU fields and their editor.
pub mod unit;

pub use records::{
    area, common_header, verify, Area, AreaKind, CommonHeader, Field, Fields, AREA_ALIGN,
    COMMON_HEADER_LEN, IMAGE_LEN, SIZE,
};
pub use store::{FruError, FruStore};
pub use unit::{
    mfg_date, parse_date, unit_field, write_unit_field, AltDate, FieldError, FieldValue, FruField,
    MfgDate,
};

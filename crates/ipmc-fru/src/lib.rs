//! ipmc-fru - the FRU Information records of FRU 0 and the scratchpad memory behind Read FRU
//! Data and Write FRU Data.
//!
//! The zero checksum and the byte builder live in ipmc-wire. The crate holds every byte layout of the FRU image and the RAM copy a Chassis Manager reads
//! and writes. It has no hardware access and no allocator, and builds for the host and for
//! thumbv7em-none-eabihf. File map:
//!
//! - `records.rs`: the common header, the board and product info areas, the compile-time image
//!   built from the IPMC_FRU_* defines, the decoder and `verify`.
//! - `store.rs`: `FruStore`, the scratchpad memory (RAM today; the flash persistence of Phase 6b
//!   lands in this file), and `FruError`.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

/// records -- FRU Information layouts, the built image, the decoder and `verify`.
pub mod records;
/// store -- the FRU scratchpad memory and the FRU error type.
pub mod store;

pub use records::{
    area, common_header, verify, Area, AreaKind, CommonHeader, Field, Fields, AREA_ALIGN,
    COMMON_HEADER_LEN, IMAGE_LEN, SIZE,
};
pub use store::{FruError, FruStore};

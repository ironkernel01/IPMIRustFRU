//! store.rs - the FRU scratchpad memory: the RAM copy of the FRU inventory area that Read FRU
//! Data and Write FRU Data (ipmc-core cmd/storage.rs) use.
//!
//! The store is RAM today: a reset restores the built image. The Phase 6b flash persistence
//! lands in this file, as a backend that the firmware's flash driver implements, so the
//! scratchpad stays in this crate and no other crate holds FRU bytes.

use crate::records::{IMAGE, SIZE};

/// FruError -- why a FRU access or decode was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FruError {
    /// The offset or the offset plus the count lies beyond the area.
    OutOfRange,
    /// The bytes are shorter than the structure being decoded.
    TooShort,
    /// A version byte is not exactly format version 1h (reserved bits 7:4 included), or a
    /// MultiRecord header's format byte is not 2h with its reserved bits 6:4 zero.
    BadVersion,
    /// An area length byte is 0 or runs past the image, or a mandatory area's offset is 00h.
    BadLength,
    /// A zero checksum does not hold (common header, area or MultiRecord record).
    BadChecksum,
    /// A board or product area has no C1h end-of-fields byte in its field list, or a MultiRecord
    /// chain has no end-of-list record.
    NoEndMarker,
    /// The common header's pad byte (byte 6) is not 00h (FRU spec Table 8-1).
    BadPad,
    /// The common header's Chassis Info Area offset is not 00h (VITA 46.11 Rule 8.1-3).
    ChassisAreaPresent,
}

impl FruError {
    /// name -- log name of the error.
    pub const fn name(self) -> &'static str {
        match self {
            FruError::OutOfRange => "OutOfRange",
            FruError::TooShort => "TooShort",
            FruError::BadVersion => "BadVersion",
            FruError::BadLength => "BadLength",
            FruError::BadChecksum => "BadChecksum",
            FruError::NoEndMarker => "NoEndMarker",
            FruError::BadPad => "BadPad",
            FruError::ChassisAreaPresent => "ChassisAreaPresent",
        }
    }
}

/// FruStore -- the RAM copy of the FRU inventory area.
pub struct FruStore {
    bytes: [u8; SIZE],
}

impl FruStore {
    /// new -- a store holding the built image.
    pub const fn new() -> FruStore {
        FruStore { bytes: IMAGE.bytes }
    }

    /// bytes -- the whole area.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// read -- `count` bytes at `offset`. Err(OutOfRange) when the range is not inside the area
    /// (reject, never clamp).
    pub fn read(&self, offset: usize, count: usize) -> Result<&[u8], FruError> {
        let end = offset.checked_add(count).ok_or(FruError::OutOfRange)?;
        self.bytes.get(offset..end).ok_or(FruError::OutOfRange)
    }

    /// contains -- the range `offset..offset + count` lies inside the area.
    pub fn contains(&self, offset: usize, count: usize) -> bool {
        self.read(offset, count).is_ok()
    }

    /// write -- stores `data` at `offset`. Err(OutOfRange) when the range is not inside the
    /// area; nothing is written then. The caller checks NVMRO (VITA Rule 8-3) first.
    pub fn write(&mut self, offset: usize, data: &[u8]) -> Result<(), FruError> {
        let end = offset.checked_add(data.len()).ok_or(FruError::OutOfRange)?;
        self.bytes
            .get_mut(offset..end)
            .ok_or(FruError::OutOfRange)?
            .copy_from_slice(data);
        Ok(())
    }
}

impl Default for FruStore {
    fn default() -> Self {
        FruStore::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_and_write_reject_out_of_range_and_change_nothing() {
        let mut store = FruStore::new();
        assert_eq!(store.read(SIZE, 1), Err(FruError::OutOfRange));
        assert_eq!(store.read(SIZE - 1, 2), Err(FruError::OutOfRange));
        assert_eq!(store.read(usize::MAX, 2), Err(FruError::OutOfRange));
        assert_eq!(store.read(SIZE, 0).map(<[u8]>::len), Ok(0));
        assert_eq!(store.write(SIZE - 1, &[1, 2]), Err(FruError::OutOfRange));
        assert_eq!(store.read(SIZE - 1, 1).unwrap(), &[0]);
        assert_eq!(store.write(SIZE - 2, &[0xAA, 0xBB]), Ok(()));
        assert_eq!(store.read(SIZE - 2, 2).unwrap(), &[0xAA, 0xBB]);
    }
}

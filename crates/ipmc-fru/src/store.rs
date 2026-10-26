//! store.rs - the FRU scratchpad memory: the RAM copy of the FRU inventory area that Read FRU
//! Data and Write FRU Data (ipmc-core cmd/storage.rs) use.
//!
//! The store is the RAM copy. `persist.rs` keeps it on flash: it loads the stored image into a
//! store at boot and saves the store after a Write FRU Data, behind the `FlashLog` trait the
//! firmware implements, so the scratchpad stays in this crate and no other crate holds FRU bytes.
//! `write` marks the store dirty; `persist::Persist::poll` is the one reader of that mark.

use crate::persist::NV_SETTINGS_MASK;
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
    /// The common header, the board area, the product area, the MultiRecord chain or the start of
    /// an Internal Use Area share a byte.
    Overlap,
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
            FruError::Overlap => "Overlap",
        }
    }
}

/// FruStore -- the RAM copy of the FRU inventory area.
pub struct FruStore {
    bytes: [u8; SIZE],
    /// An accepted write changed `bytes` since `take_dirty` last ran (written by `write`, read
    /// and cleared by `persist::Persist::poll`, both in the main loop).
    dirty: bool,
    /// The Tier-3 non-volatile settings byte (bit 0 Default-Activation-Locked, bit 1
    /// Commanded-Deactivation-Ignored), saved in the flash log record with the image: the same
    /// dirty mark and the same save rules cover it. 00h at power-up and for Tier-1 and Tier-2.
    nv_settings: u8,
}

impl FruStore {
    /// new -- a store holding the built image.
    pub const fn new() -> FruStore {
        FruStore {
            bytes: IMAGE.bytes,
            dirty: false,
            nv_settings: 0,
        }
    }

    /// from_image -- a store holding `image`, a stored image loaded from flash; not dirty.
    pub const fn from_image(image: [u8; SIZE]) -> FruStore {
        FruStore {
            bytes: image,
            dirty: false,
            nv_settings: 0,
        }
    }

    /// nv_settings -- the Tier-3 non-volatile settings byte.
    pub const fn nv_settings(&self) -> u8 {
        self.nv_settings
    }

    /// load_nv_settings -- sets the settings byte read from the flash log; not dirty.
    pub fn load_nv_settings(&mut self, settings: u8) {
        self.nv_settings = settings & NV_SETTINGS_MASK;
    }

    /// set_nv_settings -- changes the settings byte (masked with `NV_SETTINGS_MASK`); a change
    /// marks the store dirty so the flash log saves it, as an accepted Write FRU Data does. The
    /// caller checks NVMRO first.
    pub fn set_nv_settings(&mut self, settings: u8) {
        let settings = settings & NV_SETTINGS_MASK;
        if settings != self.nv_settings {
            self.nv_settings = settings;
            self.dirty = true;
        }
    }

    /// image -- the whole area as a fixed-size array (what `persist` saves).
    pub const fn image(&self) -> &[u8; SIZE] {
        &self.bytes
    }

    /// is_dirty -- a write changed the area and `persist` has not yet taken the mark.
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// take_dirty -- returns the dirty mark and clears it.
    pub fn take_dirty(&mut self) -> bool {
        core::mem::take(&mut self.dirty)
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

    /// replace_image -- stores a whole new image (the per-unit field editor, `unit.rs`) and
    /// marks the store dirty so the flash log saves it, as an accepted write does.
    pub(crate) fn replace_image(&mut self, image: [u8; SIZE]) {
        self.bytes = image;
        self.dirty = true;
    }

    /// write -- stores `data` at `offset`. Err(OutOfRange) when the range is not inside the
    /// area; nothing is written then. The caller checks NVMRO (VITA Rule 8-3) first. An accepted
    /// write marks the store dirty.
    pub fn write(&mut self, offset: usize, data: &[u8]) -> Result<(), FruError> {
        let end = offset.checked_add(data.len()).ok_or(FruError::OutOfRange)?;
        self.bytes
            .get_mut(offset..end)
            .ok_or(FruError::OutOfRange)?
            .copy_from_slice(data);
        self.dirty = true;
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
        assert!(!store.is_dirty(), "a refused write leaves the store clean");
        assert_eq!(store.write(SIZE - 2, &[0xAA, 0xBB]), Ok(()));
        assert_eq!(store.read(SIZE - 2, 2).unwrap(), &[0xAA, 0xBB]);
        assert!(store.is_dirty());
        assert!(store.take_dirty());
        assert!(!store.take_dirty(), "the mark is taken once");
        assert!(!FruStore::from_image([0; SIZE]).is_dirty());
    }
}

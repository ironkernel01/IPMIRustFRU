//! dupcache.rs - the duplicate-request cache of the link layer (PLAN.md 3.3).
//!
//! A requester that times out retries with the same sequence number, often on the other bus.
//! The cache keeps the response frame of every answered request for IPMB_DUP_WINDOW_MS (3000 ms:
//! longer than the 2750 ms requester retry window, shorter than the 3.846 s after which the live
//! Chassis Manager reuses a sequence number, so not the VITA 46.11 T9 5 s expiry; PLAN.md 3.3); a
//! repeat inside that time gets the stored frame again and no handler runs, so a retried Write
//! FRU Data or Set command takes effect once.
//!
//! Capacity: IPMB_DUP_CACHE entries of one IPMB_MAX_LEN frame plus a key (about 80 bytes each,
//! the key holds the whole request data). 64 entries cover the window at the captured peak of
//! 16 requests per second. When every entry is live the oldest is evicted and counted.
//! Time is a wrapping millisecond counter; ages use wrapping subtraction.

use crate::ipmb::{Frame, IPMB_MAX_LEN, IPMB_REQUEST_DATA_MAX};

/// DUP_CACHE -- entries, from ipmi_defs.h.
pub const DUP_CACHE: usize = ipmc_defs::IPMB_DUP_CACHE as usize;
/// DUP_WINDOW_MS -- how long an answered request stays a duplicate, from ipmi_defs.h (shorter than the
/// VITA 5 s sequence expiry: the live Chassis Manager reuses rqSeq after about 3.85 s).
pub const DUP_WINDOW_MS: u32 = ipmc_defs::IPMB_DUP_WINDOW_MS;

// The key stores the request data length in a byte.
const _: () = assert!(
    IPMB_REQUEST_DATA_MAX <= u8::MAX as usize,
    "the request data length must fit the key's length byte"
);

/// Key -- what makes two requests the same request: requester address, sequence number, NetFn,
/// command, requester LUN, responder LUN (the request's destination LUN) and the exact request
/// data (length and bytes, zero-filled past the length). Only a byte-for-byte repeat is a
/// retry: a request that reuses a sequence number with other data, or with another responder
/// LUN, is a new request and must run. A checksum or sum of the data is not enough: two writes
/// of the same bytes in another order sum alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    rq_sa: u8,
    rq_seq: u8,
    netfn: u8,
    cmd: u8,
    rq_lun: u8,
    rs_lun: u8,
    data_len: u8,
    data: [u8; IPMB_REQUEST_DATA_MAX],
}

impl Key {
    /// EMPTY -- the key of no request (unused entries).
    const EMPTY: Key = Key {
        rq_sa: 0,
        rq_seq: 0,
        netfn: 0,
        cmd: 0,
        rq_lun: 0,
        rs_lun: 0,
        data_len: 0,
        data: [0; IPMB_REQUEST_DATA_MAX],
    };

    /// of -- the key of a parsed request. None when the request data is longer than
    /// IPMB_REQUEST_DATA_MAX, which `ipmb::parse` cannot produce (the caller then runs the
    /// handler and caches nothing).
    pub fn of(request: &Frame) -> Option<Key> {
        let h = &request.header;
        let mut data = [0u8; IPMB_REQUEST_DATA_MAX];
        data.get_mut(..request.data.len())?
            .copy_from_slice(request.data);
        Some(Key {
            rq_sa: h.src,
            rq_seq: h.seq,
            netfn: h.netfn,
            cmd: h.cmd,
            rq_lun: h.src_lun,
            rs_lun: h.dest_lun,
            data_len: u8::try_from(request.data.len()).ok()?,
            data,
        })
    }
}

#[derive(Clone, Copy)]
struct Entry {
    key: Key,
    stored_ms: u32,
    len: usize,
    frame: [u8; IPMB_MAX_LEN],
    used: bool,
}

impl Entry {
    const EMPTY: Entry = Entry {
        key: Key::EMPTY,
        stored_ms: 0,
        len: 0,
        frame: [0; IPMB_MAX_LEN],
        used: false,
    };

    /// live -- true when the entry holds a response still inside the expiry.
    fn live(&self, now_ms: u32) -> bool {
        self.used && now_ms.wrapping_sub(self.stored_ms) < DUP_WINDOW_MS
    }
}

/// DupCache -- the response cache.
pub struct DupCache {
    entries: [Entry; DUP_CACHE],
    evictions: u32,
}

impl DupCache {
    /// new -- an empty cache.
    pub const fn new() -> Self {
        DupCache {
            entries: [Entry::EMPTY; DUP_CACHE],
            evictions: 0,
        }
    }

    /// lookup -- the stored response for `key` when it was stored less than DUP_WINDOW_MS ago.
    pub fn lookup(&self, key: &Key, now_ms: u32) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|e| e.live(now_ms) && e.key == *key)
            .and_then(|e| e.frame.get(..e.len))
    }

    /// insert -- stores `response` under `key`, answered at `now_ms`. The slot is the one
    /// already holding `key` (an expired repeat), else a free or expired one, else the oldest
    /// live entry, which counts as an eviction. A response longer than IPMB_MAX_LEN is not
    /// stored (a later repeat runs the handler again).
    pub fn insert(&mut self, key: Key, response: &[u8], now_ms: u32) {
        if response.len() > IPMB_MAX_LEN {
            return;
        }
        let index = self.slot_for(&key, now_ms);
        let Some(entry) = self.entries.get_mut(index) else {
            return;
        };
        if entry.live(now_ms) && entry.key != key {
            self.evictions = self.evictions.saturating_add(1);
        }
        *entry = Entry::EMPTY;
        if let Some(dst) = entry.frame.get_mut(..response.len()) {
            dst.copy_from_slice(response);
        }
        entry.len = response.len();
        entry.key = key;
        entry.stored_ms = now_ms;
        entry.used = true;
    }

    /// slot_for -- index of the entry `insert` overwrites.
    fn slot_for(&self, key: &Key, now_ms: u32) -> usize {
        let same_key = self.entries.iter().position(|e| e.used && e.key == *key);
        let reusable = || self.entries.iter().position(|e| !e.live(now_ms));
        let oldest = || {
            self.entries
                .iter()
                .enumerate()
                .max_by_key(|(_, e)| now_ms.wrapping_sub(e.stored_ms))
                .map_or(0, |(i, _)| i)
        };
        same_key.or_else(reusable).unwrap_or_else(oldest)
    }

    /// evictions -- live entries replaced because the cache was full, since start.
    pub const fn evictions(&self) -> u32 {
        self.evictions
    }
}

impl Default for DupCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipmb;

    fn key(seq: u8) -> Key {
        Key {
            rq_sa: 0x20,
            rq_seq: seq,
            ..Key::EMPTY
        }
    }

    /// Frame of a request with the given LUNs and data, for the key tests.
    fn request(dest_lun: u8, data: &[u8]) -> Frame<'_> {
        Frame {
            header: ipmb::Header {
                dest: 0x82,
                netfn: 0x0A,
                dest_lun,
                src: 0x20,
                seq: 7,
                src_lun: 0,
                cmd: 0x12,
            },
            completion_code: None,
            data,
        }
    }

    #[test]
    fn the_key_holds_the_lun_and_the_exact_data() {
        let base = Key::of(&request(0, &[0x00, 0xC0, 0x00, 0x01, 0x02])).unwrap();
        assert_eq!(
            base,
            Key::of(&request(0, &[0x00, 0xC0, 0x00, 0x01, 0x02])).unwrap()
        );
        // The same bytes in another order sum alike and are another request.
        let swapped = Key::of(&request(0, &[0x00, 0xC0, 0x00, 0x02, 0x01])).unwrap();
        assert_ne!(base, swapped);
        // The responder LUN is part of the key.
        assert_ne!(
            base,
            Key::of(&request(1, &[0x00, 0xC0, 0x00, 0x01, 0x02])).unwrap()
        );
        // A zero byte more is a longer request, not the same one.
        assert_ne!(
            Key::of(&request(0, &[1, 2])).unwrap(),
            Key::of(&request(0, &[1, 2, 0])).unwrap()
        );
        // Data past the largest request is refused, not clamped.
        let too_long = [0u8; IPMB_REQUEST_DATA_MAX + 1];
        assert_eq!(Key::of(&request(0, &too_long)), None);
        assert!(Key::of(&request(0, &too_long[..IPMB_REQUEST_DATA_MAX])).is_some());
    }

    #[test]
    fn hit_inside_the_expiry_miss_at_it() {
        let mut c = DupCache::new();
        c.insert(key(1), &[1, 2, 3], 1000);
        assert_eq!(c.lookup(&key(1), 1000), Some(&[1u8, 2, 3][..]));
        assert_eq!(
            c.lookup(&key(1), 1000 + DUP_WINDOW_MS - 1),
            Some(&[1u8, 2, 3][..])
        );
        assert_eq!(c.lookup(&key(1), 1000 + DUP_WINDOW_MS), None);
        assert_eq!(c.lookup(&key(2), 1000), None);
    }

    #[test]
    fn expiry_survives_counter_wrap() {
        let mut c = DupCache::new();
        let t0 = u32::MAX - 10;
        c.insert(key(1), &[9], t0);
        assert!(c.lookup(&key(1), t0.wrapping_add(100)).is_some());
        assert!(c.lookup(&key(1), t0.wrapping_add(DUP_WINDOW_MS)).is_none());
    }

    #[test]
    fn full_cache_evicts_the_oldest_and_counts() {
        let mut c = DupCache::new();
        for i in 0..DUP_CACHE {
            c.insert(key(i as u8), &[i as u8], 100 + i as u32);
        }
        assert_eq!(c.evictions(), 0);
        // One key past the cache size, so it is new whatever IPMB_DUP_CACHE is.
        let newest = key(DUP_CACHE as u8);
        c.insert(newest, &[0xEE], 200);
        assert_eq!(c.evictions(), 1);
        assert!(c.lookup(&key(0), 200).is_none(), "oldest is gone");
        for i in 1..DUP_CACHE {
            assert!(c.lookup(&key(i as u8), 200).is_some(), "entry {i} kept");
        }
        assert!(c.lookup(&newest, 200).is_some());
    }

    #[test]
    fn expired_entries_are_reused_without_an_eviction() {
        let mut c = DupCache::new();
        for i in 0..DUP_CACHE {
            c.insert(key(i as u8), &[i as u8], 0);
        }
        c.insert(key(DUP_CACHE as u8), &[0xEE], DUP_WINDOW_MS);
        assert_eq!(c.evictions(), 0);
    }

    #[test]
    fn a_repeat_after_expiry_reuses_its_own_slot() {
        let mut c = DupCache::new();
        c.insert(key(1), &[1], 0);
        c.insert(key(1), &[2], DUP_WINDOW_MS + 1);
        assert_eq!(c.lookup(&key(1), DUP_WINDOW_MS + 1), Some(&[2u8][..]));
        assert_eq!(c.evictions(), 0);
    }
}

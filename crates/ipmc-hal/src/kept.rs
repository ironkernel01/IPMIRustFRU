// kept.rs - words that survive a software reset and the checked record built on them.

/// KeptWords -- N words that survive a software reset (NOINIT RAM on the F401).
pub trait KeptWords<const N: usize> {
    /// read -- the N words, whatever the RAM holds.
    fn read(&self) -> [u32; N];
    /// write -- stores the N words in index order (a check word goes last).
    fn write(&mut self, words: [u32; N]);
    /// clear -- writes 0 to word `index`.
    fn clear(&mut self, index: usize);
}

/// Plain memory: an array of words is a `KeptWords` (the host tests and the emulator keep the words
/// in one).
impl<const N: usize> KeptWords<N> for [u32; N] {
    fn read(&self) -> [u32; N] {
        *self
    }

    fn write(&mut self, words: [u32; N]) {
        *self = words;
    }

    fn clear(&mut self, index: usize) {
        if let Some(word) = self.get_mut(index) {
            *word = 0;
        }
    }
}

/// CHECKED_DATA -- data words of a checked record; the check word follows them.
pub const CHECKED_DATA: usize = 3;

/// store_checked -- the three data words and their check word `ipmc_wire::check_word(data, magic)`.
/// No panic path and no `core::fmt`: the fault handlers call it.
pub fn store_checked<K: KeptWords<4>>(kept: &mut K, data: [u32; CHECKED_DATA], magic: u32) {
    let [a, b, c] = data;
    kept.write([a, b, c, ipmc_wire::check_word(data, magic)]);
}

/// take_checked -- reads the four words, clears the check word, and returns the data words when
/// the check word matches `magic` (None otherwise).
pub fn take_checked<K: KeptWords<4>>(kept: &mut K, magic: u32) -> Option<[u32; CHECKED_DATA]> {
    let [a, b, c, check] = kept.read();
    kept.clear(CHECKED_DATA);
    let data = [a, b, c];
    (check == ipmc_wire::check_word(data, magic)).then_some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAGIC: u32 = 0x5EC0_5EC0;

    #[test]
    fn store_then_take_returns_the_data_once() {
        let mut ram = [0u32; 4];
        store_checked(&mut ram, [1, 2, 3], MAGIC);
        assert_eq!(ram[3], ipmc_wire::check_word([1, 2, 3], MAGIC));
        assert_eq!(take_checked(&mut ram, MAGIC), Some([1, 2, 3]));
        assert_eq!(ram[3], 0, "the check word is cleared");
        assert_eq!(take_checked(&mut ram, MAGIC), None);
    }

    #[test]
    fn wrong_magic_and_random_ram_give_none() {
        let mut ram = [0u32; 4];
        store_checked(&mut ram, [7, 8, 9], MAGIC);
        assert_eq!(take_checked(&mut ram, MAGIC ^ 1), None);
        let mut noise = [0xDEAD_BEEF, 1, 2, 3];
        assert_eq!(take_checked(&mut noise, MAGIC), None);
    }
}

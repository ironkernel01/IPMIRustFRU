//! request - the boot request words that cross a software reset (design 7.1).
//!
//! The runtime writes four words at the fixed NOINIT address; the bootloader reads them after
//! the reset and accepts them only after a software reset. Drift failure mode: a runtime and a
//! bootloader built with different words disagree, the request is refused, and the board starts
//! the runtime.

use ipmc_wire::check_word;

/// MAGIC -- word 0 of a request.
pub const MAGIC: u32 = 0xB007_5EC7;
/// CHECK_MAGIC -- XORed with words 0 to 2 into word 3, so a stale RAM pattern does not pass.
pub const CHECK_MAGIC: u32 = 0x5A17_B007;
/// WORDS -- words in a request.
pub const WORDS: usize = 4;

/// Interface -- which interface asked for the bootloader (word 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interface {
    Console = 1,
    App = 2,
}

impl Interface {
    /// text -- the interface as the start-up trace names it.
    pub const fn text(self) -> &'static str {
        match self {
            Interface::Console => "console",
            Interface::App => "app port",
        }
    }
}

/// encode -- the four words of a request: magic, interface, reserved 0, check word.
pub const fn encode(interface: Interface) -> [u32; WORDS] {
    let data = [MAGIC, interface as u32, 0];
    [data[0], data[1], data[2], check_word(data, CHECK_MAGIC)]
}

/// decode -- Some(interface) when the words are a valid request and the reset was a software
/// reset; None otherwise.
pub fn decode(words: [u32; WORDS], software_reset: bool) -> Option<Interface> {
    let [magic, interface, reserved, check] = words;
    if !software_reset
        || magic != MAGIC
        || check != check_word([magic, interface, reserved], CHECK_MAGIC)
    {
        return None;
    }
    match interface {
        1 => Some(Interface::Console),
        2 => Some(Interface::App),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_then_decode_for_both_interfaces() {
        for interface in [Interface::Console, Interface::App] {
            assert_eq!(decode(encode(interface), true), Some(interface));
        }
        assert_eq!(Interface::Console.text(), "console");
        assert_eq!(Interface::App.text(), "app port");
    }

    #[test]
    fn invalid_requests_are_refused() {
        let good = encode(Interface::Console);
        let mut wrong_magic = good;
        wrong_magic[0] ^= 1;
        let mut wrong_check = good;
        wrong_check[3] ^= 1;
        let mut unknown = [MAGIC, 3, 0, 0];
        unknown[3] = check_word([MAGIC, 3, 0], CHECK_MAGIC);
        let mut zero_interface = [MAGIC, 0, 0, 0];
        zero_interface[3] = check_word([MAGIC, 0, 0], CHECK_MAGIC);
        for words in [
            wrong_magic,
            wrong_check,
            unknown,
            zero_interface,
            [0; WORDS],
        ] {
            assert_eq!(decode(words, true), None, "{words:x?}");
        }
    }

    #[test]
    fn a_request_without_a_software_reset_is_refused() {
        assert_eq!(decode(encode(Interface::App), false), None);
    }
}

//! error - the ERR codes of the session (design 8.5), one enum with the answer-line writer.

use crate::header::ImageError;
use crate::text::{TextOut, CRLF};
use ipmc_fru::persist::FlashError;

/// BootError -- why a command or a LOAD record is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootError {
    Format,
    Checksum,
    Type,
    Wrap,
    Range,
    NotErased,
    Verify,
    Flash(FlashError),
    Nvmro,
    LineTooLong,
    Aborted,
    Timeout,
    /// LOAD before an ERASE APP in this session.
    NotErasedFirst,
    Cancelled,
    NothingPending,
    Image(ImageError),
    /// A word that is no command.
    Unknown,
    /// A known command word with other words than its row has.
    Args,
}

impl From<FlashError> for BootError {
    /// NVMRO asserted is its own code; every other fault is `FLASH <fault>`.
    fn from(error: FlashError) -> Self {
        match error {
            FlashError::NvmroAsserted => BootError::Nvmro,
            other => BootError::Flash(other),
        }
    }
}

impl BootError {
    /// code -- the word after ERR; NOT_ERASED names both NotErased and NotErasedFirst.
    pub const fn code(self) -> &'static str {
        match self {
            BootError::Format => "FORMAT",
            BootError::Checksum => "CHECKSUM",
            BootError::Type => "TYPE",
            BootError::Wrap => "WRAP",
            BootError::Range => "RANGE",
            BootError::NotErased | BootError::NotErasedFirst => "NOT_ERASED",
            BootError::Verify => "VERIFY",
            BootError::Flash(_) => "FLASH",
            BootError::Nvmro => "NVMRO",
            BootError::LineTooLong => "LINE_TOO_LONG",
            BootError::Aborted => "ABORTED",
            BootError::Timeout => "TIMEOUT",
            BootError::Cancelled => "CANCELLED",
            BootError::NothingPending => "NOTHING_PENDING",
            BootError::Image(_) => "IMAGE",
            BootError::Unknown => "UNKNOWN",
            BootError::Args => "ARGS",
        }
    }
}

/// put_err_head -- `ERR <CODE>` then, where the design gives one, a space and the detail (Flash:
/// the fault name, Image: the reason, NotErasedFirst: `erase APP first`); no line end, so a caller
/// can add more words.
pub fn put_err_head(out: &mut dyn TextOut, error: BootError) {
    out.put("ERR ");
    out.put(error.code());
    let detail = match error {
        BootError::Flash(fault) => Some(fault.name()),
        BootError::Image(reason) => Some(reason.name()),
        BootError::NotErasedFirst => Some("erase APP first"),
        _ => None,
    };
    if let Some(detail) = detail {
        out.put(" ");
        out.put(detail);
    }
}

/// put_err -- the whole answer line: the head, then CR LF.
pub fn put_err(out: &mut dyn TextOut, error: BootError) {
    put_err_head(out, error);
    out.put(CRLF);
}

/// put_err_word -- the answer line of `Unknown` and `Args`: `ERR <CODE> <word>`, CR LF.
pub fn put_err_word(out: &mut dyn TextOut, error: BootError, word: &str) {
    put_err_head(out, error);
    out.put(" ");
    out.put(word);
    out.put(CRLF);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;

    #[test]
    fn answer_lines() {
        let rows: [(BootError, &str); 19] = [
            (BootError::Format, "ERR FORMAT\r\n"),
            (BootError::Checksum, "ERR CHECKSUM\r\n"),
            (BootError::Type, "ERR TYPE\r\n"),
            (BootError::Wrap, "ERR WRAP\r\n"),
            (BootError::Range, "ERR RANGE\r\n"),
            (BootError::NotErased, "ERR NOT_ERASED\r\n"),
            (
                BootError::NotErasedFirst,
                "ERR NOT_ERASED erase APP first\r\n",
            ),
            (BootError::Verify, "ERR VERIFY\r\n"),
            (
                BootError::Flash(FlashError::Programming),
                "ERR FLASH Programming\r\n",
            ),
            (BootError::Nvmro, "ERR NVMRO\r\n"),
            (BootError::LineTooLong, "ERR LINE_TOO_LONG\r\n"),
            (BootError::Aborted, "ERR ABORTED\r\n"),
            (BootError::Timeout, "ERR TIMEOUT\r\n"),
            (BootError::Cancelled, "ERR CANCELLED\r\n"),
            (BootError::NothingPending, "ERR NOTHING_PENDING\r\n"),
            (
                BootError::Image(ImageError::DigestMismatch),
                "ERR IMAGE digest mismatch\r\n",
            ),
            (
                BootError::Image(ImageError::Missing),
                "ERR IMAGE missing\r\n",
            ),
            (BootError::Unknown, "ERR UNKNOWN\r\n"),
            (BootError::Args, "ERR ARGS\r\n"),
        ];
        for (error, want) in rows {
            let mut out = String::new();
            put_err(&mut out, error);
            assert_eq!(out, want);
        }
    }

    #[test]
    fn word_answers() {
        let mut out = String::new();
        put_err_word(&mut out, BootError::Unknown, "FOO");
        put_err_word(&mut out, BootError::Args, "RUN");
        assert_eq!(out, "ERR UNKNOWN FOO\r\nERR ARGS RUN\r\n");
    }

    #[test]
    fn nvmro_maps_from_the_register_layer() {
        assert_eq!(BootError::from(FlashError::NvmroAsserted), BootError::Nvmro);
        assert_eq!(
            BootError::from(FlashError::Timeout),
            BootError::Flash(FlashError::Timeout)
        );
    }
}

// args.rs - parameter tokens handed to a command handler, with typed accessors.
//
// Positions count parameters only: index 0 is the first `[REQUIRED]` or `{OPTIONAL}` token of
// the matched syntax, whatever its place in the line. No accessor panics.

use crate::MENU_TOKENS_MAX;

/// ArgError -- why a parameter could not be read. Variants name the fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgError {
    /// No parameter at this position (an optional parameter left out).
    Missing,
    /// The token is not a number in the accepted form.
    NotANumber,
    /// The number does not fit the requested type.
    Overflow,
    /// The token is neither `on` nor `off`.
    NotOnOff,
}

impl ArgError {
    /// name -- log name of the argument error.
    pub fn name(self) -> &'static str {
        match self {
            ArgError::Missing => "missing parameter",
            ArgError::NotANumber => "not a number",
            ArgError::Overflow => "number out of range",
            ArgError::NotOnOff => "expected ON or OFF",
        }
    }
}

/// optional -- turns `Missing` into `Ok(None)` so an optional parameter reads in one call:
/// `optional(args.u32_at(1))?`. Any other error is kept.
pub fn optional<T>(result: Result<T, ArgError>) -> Result<Option<T>, ArgError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(ArgError::Missing) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Args -- the parameter tokens of one matched command. Borrows the input line.
#[derive(Clone, Copy, Debug)]
pub struct Args<'a> {
    tokens: [&'a str; MENU_TOKENS_MAX],
    count: usize,
}

impl<'a> Args<'a> {
    /// empty -- no parameters.
    pub const fn empty() -> Self {
        Args {
            tokens: [""; MENU_TOKENS_MAX],
            count: 0,
        }
    }

    /// push -- appends one parameter. Returns false (and stores nothing) when the capacity is full.
    pub(crate) fn push(&mut self, token: &'a str) -> bool {
        match self.tokens.get_mut(self.count) {
            Some(slot) => {
                *slot = token;
                self.count += 1;
                true
            }
            None => false,
        }
    }

    /// from_slice -- builds Args from a token slice; None when it holds more than `MENU_TOKENS_MAX`.
    pub fn from_slice(tokens: &[&'a str]) -> Option<Self> {
        let mut args = Args::empty();
        for token in tokens {
            if !args.push(token) {
                return None;
            }
        }
        Some(args)
    }

    /// len -- number of parameters present.
    pub fn len(&self) -> usize {
        self.count
    }

    /// is_empty -- true when no parameter is present.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// get -- the parameter at `index`, or None.
    pub fn get(&self, index: usize) -> Option<&'a str> {
        if index < self.count {
            self.tokens.get(index).copied()
        } else {
            None
        }
    }

    /// text_at -- the parameter as written. `Err(Missing)` when absent.
    pub fn text_at(&self, index: usize) -> Result<&'a str, ArgError> {
        self.get(index).ok_or(ArgError::Missing)
    }

    /// u32_at -- unsigned decimal (`42`) or hexadecimal (`0x2A`, prefix case-insensitive).
    /// No sign, no separators. `Err(NotANumber)` for any other form, `Err(Overflow)` past 32 bits.
    pub fn u32_at(&self, index: usize) -> Result<u32, ArgError> {
        parse_u32(self.text_at(index)?)
    }

    /// i32_at -- decimal with an optional leading `-`. No `+`, no hexadecimal.
    pub fn i32_at(&self, index: usize) -> Result<i32, ArgError> {
        parse_i32(self.text_at(index)?)
    }

    /// on_off_at -- `on` is true, `off` is false (case-insensitive).
    pub fn on_off_at(&self, index: usize) -> Result<bool, ArgError> {
        let text = self.text_at(index)?;
        if crate::syntax::eq_ignore_case(text, "on") {
            Ok(true)
        } else if crate::syntax::eq_ignore_case(text, "off") {
            Ok(false)
        } else {
            Err(ArgError::NotOnOff)
        }
    }
}

/// Digits-only check, then the standard parser; once every byte is a digit the only remaining
/// failure is overflow.
fn parse_digits(digits: &str, radix: u32) -> Result<u32, ArgError> {
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return Err(ArgError::NotANumber);
    }
    u32::from_str_radix(digits, radix).map_err(|_| ArgError::Overflow)
}

fn parse_u32(text: &str) -> Result<u32, ArgError> {
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => parse_digits(hex, 16),
        None => parse_digits(text, 10),
    }
}

fn parse_i32(text: &str) -> Result<i32, ArgError> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let magnitude = parse_digits(digits, 10)?;
    if negative {
        // i32::MIN has magnitude 2^31, one more than i32::MAX.
        let signed = i64::from(magnitude);
        i32::try_from(-signed).map_err(|_| ArgError::Overflow)
    } else {
        i32::try_from(magnitude).map_err(|_| ArgError::Overflow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &'static str) -> Args<'static> {
        Args::from_slice(&[text]).unwrap()
    }

    #[test]
    fn u32_decimal_and_hex() {
        assert_eq!(one("42").u32_at(0), Ok(42));
        assert_eq!(one("0x2A").u32_at(0), Ok(42));
        assert_eq!(one("0X2a").u32_at(0), Ok(42));
        assert_eq!(one("4294967295").u32_at(0), Ok(u32::MAX));
        assert_eq!(one("0xFFFFFFFF").u32_at(0), Ok(u32::MAX));
    }

    #[test]
    fn u32_rejects_bad_forms() {
        for bad in ["", "-1", "+1", "1.5", "0x", "0xG", "abc", "1_000", " 1"] {
            assert_eq!(one(bad).u32_at(0), Err(ArgError::NotANumber), "{bad:?}");
        }
        assert_eq!(one("4294967296").u32_at(0), Err(ArgError::Overflow));
        assert_eq!(one("0x100000000").u32_at(0), Err(ArgError::Overflow));
    }

    #[test]
    fn i32_range_and_sign() {
        assert_eq!(one("-5").i32_at(0), Ok(-5));
        assert_eq!(one("7").i32_at(0), Ok(7));
        assert_eq!(one("2147483647").i32_at(0), Ok(i32::MAX));
        assert_eq!(one("-2147483648").i32_at(0), Ok(i32::MIN));
        assert_eq!(one("2147483648").i32_at(0), Err(ArgError::Overflow));
        assert_eq!(one("-2147483649").i32_at(0), Err(ArgError::Overflow));
        assert_eq!(one("+5").i32_at(0), Err(ArgError::NotANumber));
        assert_eq!(one("-").i32_at(0), Err(ArgError::NotANumber));
        assert_eq!(one("0x10").i32_at(0), Err(ArgError::NotANumber));
    }

    #[test]
    fn on_off() {
        assert_eq!(one("ON").on_off_at(0), Ok(true));
        assert_eq!(one("off").on_off_at(0), Ok(false));
        assert_eq!(one("maybe").on_off_at(0), Err(ArgError::NotOnOff));
        assert_eq!(one("o").on_off_at(0), Err(ArgError::NotOnOff));
    }

    #[test]
    fn missing_and_optional() {
        let args = one("1");
        assert_eq!(args.text_at(1), Err(ArgError::Missing));
        assert_eq!(args.u32_at(5), Err(ArgError::Missing));
        assert_eq!(optional(args.u32_at(1)), Ok(None));
        assert_eq!(optional(args.u32_at(0)), Ok(Some(1)));
        assert_eq!(optional(one("x").u32_at(0)), Err(ArgError::NotANumber));
        assert_eq!(args.len(), 1);
        assert!(!args.is_empty());
        assert!(Args::empty().is_empty());
    }

    #[test]
    fn capacity_is_bounded() {
        let many = ["x"; MENU_TOKENS_MAX + 1];
        assert!(Args::from_slice(&many[..MENU_TOKENS_MAX]).is_some());
        assert!(Args::from_slice(&many).is_none());
    }
}

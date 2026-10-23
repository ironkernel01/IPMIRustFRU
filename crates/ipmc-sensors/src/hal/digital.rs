//! digital.rs - the interface for digital input levels.

use super::SourceError;

/// Input -- the digital inputs of the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// Supplies good, active high.
    Pgood,
    /// Supply fault, active low.
    FaultN,
    /// Board over-temperature, active low.
    OvertempN,
    /// Payload held in reset, active low.
    PayloadResetN,
}

impl Input {
    /// COUNT -- number of inputs.
    pub const COUNT: usize = 4;
    /// ALL -- every input, in `index` order.
    pub const ALL: [Input; Input::COUNT] = [
        Input::Pgood,
        Input::FaultN,
        Input::OvertempN,
        Input::PayloadResetN,
    ];

    /// index -- position in `ALL`, for arrays indexed by input.
    pub const fn index(self) -> usize {
        match self {
            Input::Pgood => 0,
            Input::FaultN => 1,
            Input::OvertempN => 2,
            Input::PayloadResetN => 3,
        }
    }
}

/// DigitalInputs -- pin levels.
pub trait DigitalInputs {
    /// level_high -- true when the pin reads high (the electrical level, not "asserted").
    fn level_high(&mut self, input: Input) -> Result<bool, SourceError>;
}

/// DeadInput -- test inputs that answer high except `dead`, which fails with `error`.
#[cfg(test)]
pub(crate) struct DeadInput {
    pub dead: Input,
    pub error: SourceError,
}

#[cfg(test)]
impl DigitalInputs for DeadInput {
    fn level_high(&mut self, input: Input) -> Result<bool, SourceError> {
        if input == self.dead {
            Err(self.error)
        } else {
            Ok(true)
        }
    }
}

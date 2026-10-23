//! power.rs - the simulated enable output (implements `PowerSwitch`).

use crate::hal::{PowerSwitch, SourceError};

/// SimPower -- remembers the commanded level, drives nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimPower {
    enabled: bool,
}

impl SimPower {
    /// new -- a simulated output that starts off.
    pub const fn new() -> Self {
        SimPower { enabled: false }
    }
}

impl Default for SimPower {
    fn default() -> Self {
        SimPower::new()
    }
}

impl PowerSwitch for SimPower {
    fn set_enable(&mut self, on: bool) -> Result<(), SourceError> {
        self.enabled = on;
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_remembers_the_commanded_level() {
        let mut power = SimPower::new();
        assert!(!power.is_enabled());
        assert_eq!(power.set_enable(true), Ok(()));
        assert!(power.is_enabled());
        assert_eq!(power.set_enable(false), Ok(()));
        assert!(!power.is_enabled());
    }
}

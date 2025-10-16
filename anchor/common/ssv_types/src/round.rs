use std::{
    fmt,
    fmt::{Display, Formatter},
    num::NonZeroUsize,
    ops::Add,
};

use derive_more::Deref;

/// This represents an individual round, these change on regular time intervals
#[derive(Clone, Copy, Debug, Deref, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Round(NonZeroUsize);

impl From<u64> for Round {
    fn from(round: u64) -> Round {
        Round(NonZeroUsize::new(round as usize).expect("round == 0"))
    }
}

impl From<Round> for u64 {
    fn from(round: Round) -> u64 {
        round.0.get() as u64
    }
}

impl Add<u64> for Round {
    type Output = Option<Round>;

    /// Adds a u64 to a Round using checked arithmetic to prevent overflow.
    ///
    /// Returns None if:
    /// - The addition would overflow (self + rhs > usize::MAX)
    /// - The result would be zero (handled by NonZeroUsize::new)
    ///
    /// This prevents security vulnerabilities where overflow could cause:
    /// - Message validation to accept messages from any round
    /// - Message container iteration to process all rounds instead of just future ones
    fn add(self, rhs: u64) -> Option<Round> {
        self.0
            .get()
            .checked_add(rhs as usize)
            .and_then(NonZeroUsize::new)
            .map(Round)
    }
}

impl Default for Round {
    fn default() -> Self {
        // rounds are indexed starting at 1
        Round(NonZeroUsize::new(1).expect("1 != 0"))
    }
}

impl Display for Round {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Round {
    /// Returns the next round
    pub fn next(&self) -> Option<Round> {
        self.0.checked_add(1).map(Round)
    }

    /// Sets the current round
    pub fn set(&mut self, round: Round) {
        *self = round;
    }
}

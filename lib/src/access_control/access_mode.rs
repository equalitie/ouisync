use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use thiserror::Error;

/// Access mode of a repository.
#[derive(
    Clone,
    Copy,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Debug,
    Serialize,
    Deserialize,
    IntoPrimitive,
    TryFromPrimitive,
)]
#[repr(u8)]
#[serde(into = "u8", try_from = "u8")]
pub enum AccessMode {
    /// Repository is neither readable not writtable (but can still be synced).
    Blind = 0,
    /// Repository is readable but not writtable.
    Read = 1,
    /// Repository is both readable and writable.
    Write = 2,
}

impl AccessMode {
    pub fn can_read(&self) -> bool {
        self != &Self::Blind
    }
}

impl FromStr for AccessMode {
    type Err = AccessModeParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.chars().next() {
            Some('b' | 'B') => Ok(AccessMode::Blind),
            Some('r' | 'R') => Ok(AccessMode::Read),
            Some('w' | 'W') => Ok(AccessMode::Write),
            _ => Err(AccessModeParseError),
        }
    }
}

impl fmt::Display for AccessMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blind => write!(f, "blind"),
            Self::Read => write!(f, "read"),
            Self::Write => write!(f, "write"),
        }
    }
}

#[derive(Debug, Error)]
#[error("failed to parse access mode")]
pub struct AccessModeParseError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ord() {
        assert!(AccessMode::Blind < AccessMode::Read);
        assert!(AccessMode::Blind < AccessMode::Write);
        assert!(AccessMode::Read < AccessMode::Write);
    }
}

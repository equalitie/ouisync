use super::DecodeError;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use thiserror::Error;

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[repr(u8)]
#[serde(into = "u8", try_from = "u8")]
pub enum AccessMode {
    Blind = 0,
    Read = 1,
    Write = 2,
}

impl TryFrom<u8> for AccessMode {
    type Error = DecodeError;

    fn try_from(byte: u8) -> Result<Self, Self::Error> {
        match byte {
            b if b == Self::Blind as u8 => Ok(Self::Blind),
            b if b == Self::Read as u8 => Ok(Self::Read),
            b if b == Self::Write as u8 => Ok(Self::Write),
            _ => Err(DecodeError),
        }
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

impl From<AccessMode> for u8 {
    fn from(mode: AccessMode) -> Self {
        mode as u8
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

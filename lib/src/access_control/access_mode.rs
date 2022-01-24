use super::DecodeError;
use std::str::FromStr;
use thiserror::Error;

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
#[repr(u8)]
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

#[derive(Debug, Error)]
#[error("failed to parse access mode")]
pub struct AccessModeParseError;

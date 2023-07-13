use crate::protocol::BLOCK_RECORD_SIZE;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Strongly typed storage size.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StorageSize {
    bytes: u64,
}

impl StorageSize {
    pub fn from_bytes(value: u64) -> Self {
        Self { bytes: value }
    }

    pub fn from_blocks(value: u64) -> Self {
        Self {
            bytes: value * BLOCK_RECORD_SIZE,
        }
    }

    pub fn to_bytes(self) -> u64 {
        self.bytes
    }

    // pub fn to_blocks(self) -> u64 {
    //     self.bytes / BLOCK_RECORD_SIZE
    // }

    pub fn saturating_sub(self, rhs: Self) -> Self {
        Self {
            bytes: self.bytes.saturating_sub(rhs.bytes),
        }
    }
}

impl fmt::Display for StorageSize {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        const SUFFIXES: &[(u64, &str)] = &[
            (1024, "ki"),
            (1024 * 1024, "Mi"),
            (1024 * 1024 * 1024, "Gi"),
            (1024 * 1024 * 1024 * 1024, "Ti"),
        ];

        for (value, suffix) in SUFFIXES.iter().rev().copied() {
            if self.bytes >= value {
                return write!(f, "{:.2} {}B", self.bytes as f64 / value as f64, suffix);
            }
        }

        write!(f, "{} B", self.bytes)
    }
}

impl FromStr for StorageSize {
    type Err = parse_size::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            bytes: parse_size::parse_size(s)?,
        })
    }
}

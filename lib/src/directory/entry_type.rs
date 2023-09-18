use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::{Deserialize, Serialize};

/// Type of filesystem entry.
#[derive(
    Clone,
    Copy,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    Debug,
    Deserialize,
    Serialize,
    IntoPrimitive,
    TryFromPrimitive,
)]
#[repr(u8)]
pub enum EntryType {
    File = 1,
    Directory = 2,
}

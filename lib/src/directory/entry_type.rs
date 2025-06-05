use num_enum::{IntoPrimitive, TryFromPrimitive};
use ouisync_macros::api;
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
#[serde(into = "u8", try_from = "u8")]
#[api]
pub enum EntryType {
    File = 1,
    Directory = 2,
}

//! Time utilities

use std::{
    fmt,
    time::{Duration, SystemTime},
};

/// Returns the number of milliseconds since the unix epoch until the given time or error if time is
/// out of range.
pub(crate) fn to_millis_since_epoch(time: SystemTime) -> Result<u64, TimeOutOfRange> {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| TimeOutOfRange)?
        .as_millis()
        .try_into()
        .map_err(|_| TimeOutOfRange)
}

/// Returns the time corresponding to the given number of milliseconds since the unix epoch.
pub(crate) fn from_millis_since_epoch(ms: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(ms)
}

#[derive(Debug)]
pub(crate) struct TimeOutOfRange;

impl fmt::Display for TimeOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "time out of range")
    }
}

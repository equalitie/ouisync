//! Utilities for deadlock detection

pub mod asynch;
pub mod blocking;

mod timer;
mod tracker;

use std::time::Duration;

const WARNING_TIMEOUT: Duration = Duration::from_secs(5);

//! Utilities for deadlock detection

pub mod blocking;

mod async_mutex;
mod expect_short_lifetime;
mod timer;

pub use self::async_mutex::{Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard};
pub(crate) use self::expect_short_lifetime::ExpectShortLifetime;

use std::time::Duration;

const WARNING_TIMEOUT: Duration = Duration::from_secs(5);

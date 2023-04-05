//! Utilities for deadlock detection

mod async_mutex;
mod blocking;
mod expect_short_lifetime;
mod timer;

pub(crate) use self::expect_short_lifetime::ExpectShortLifetime;
pub use self::{
    async_mutex::{Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard},
    blocking::{
        Mutex as BlockingMutex, MutexGuard as BlockingMutexGuard, RwLock as BlockingRwLock,
        RwLockReadGuard as BlockingRwLockReadGuard, RwLockWriteGuard as BlockingRwLockWriteGuard,
    },
};

use std::time::Duration;

const WARNING_TIMEOUT: Duration = Duration::from_secs(5);

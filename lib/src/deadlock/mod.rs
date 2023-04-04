//! Utilities for deadlock detection

pub mod blocking;

mod async_mutex;
mod expect_short_lifetime;
mod timer;

pub use self::async_mutex::{Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard};
pub(crate) use self::expect_short_lifetime::ExpectShortLifetime;

use std::{backtrace::Backtrace, fmt, panic::Location, time::Duration};

const WARNING_TIMEOUT: Duration = Duration::from_secs(5);

struct Context {
    location: &'static Location<'static>,
    backtrace: Backtrace,
}

impl Context {
    fn new(location: &'static Location<'static>) -> Self {
        Self {
            location,
            backtrace: Backtrace::capture(),
        }
    }
}

impl fmt::Display for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n{}", self.location, self.backtrace)
    }
}

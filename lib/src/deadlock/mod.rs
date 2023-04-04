//! Utilities for deadlock detection

pub mod blocking;

mod async_mutex;
mod expect_short_lifetime;
mod timer;

use once_cell::sync::Lazy;

pub use self::async_mutex::{Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard};
pub(crate) use self::expect_short_lifetime::ExpectShortLifetime;
use self::timer::{Id, Timer};

use std::{
    backtrace::Backtrace,
    fmt,
    panic::Location,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const WARNING_TIMEOUT: Duration = Duration::from_secs(5);

struct Context {
    start_time: Instant,
    location: &'static Location<'static>,
    backtrace: Backtrace,
}

impl Context {
    fn new(location: &'static Location<'static>) -> Self {
        Self {
            start_time: Instant::now(),
            location,
            backtrace: Backtrace::capture(),
        }
    }
}

impl fmt::Display for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "elapsed: {:?}, started at: {}\nbacktrace:\n{}",
            self.start_time.elapsed(),
            self.location,
            self.backtrace
        )
    }
}

static TIMER: Timer<Context> = Timer::new();
static WATCHING_THREAD: Lazy<JoinHandle<()>> = Lazy::new(|| thread::spawn(watching_thread));

fn schedule(duration: Duration, context: Context) -> Id {
    // Make sure the thread is instantiated.
    let _ = *WATCHING_THREAD;
    let deadline = Instant::now() + duration;

    TIMER.schedule(deadline, context)
}

fn cancel(id: Id) {
    if TIMER.cancel(id).is_none() {
        println!("Previously reported task (id: {}) eventually completed", id);
    }
}

fn watching_thread() {
    loop {
        let (id, context) = TIMER.wait();

        // Using `println!` and not `tracing::*` to avoid circular dependencies because on
        // Android tracing uses `StateMonitor` which uses these mutexes.
        println!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
        println!("Task taking too long (id: {})\n{}\n", id, context);
        println!("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!");
    }
}

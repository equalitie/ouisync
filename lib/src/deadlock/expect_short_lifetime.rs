use super::timer::{Id, Timer};
use once_cell::sync::Lazy;
use std::{
    backtrace::Backtrace,
    fmt,
    panic::Location,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Attach this to objects that are expected to be short-lived to be warned when they live longer
/// than expected.
pub(crate) struct ExpectShortLifetime {
    id: Id,
}

impl ExpectShortLifetime {
    #[track_caller]
    pub fn new(max_lifetime: Duration) -> Self {
        Self::new_in(max_lifetime, Location::caller())
    }

    pub fn new_in(max_lifetime: Duration, location: &'static Location<'static>) -> Self {
        let context = Context::new(location);
        let id = schedule(max_lifetime, context);

        Self { id }
    }
}

impl Drop for ExpectShortLifetime {
    fn drop(&mut self) {
        cancel(self.id);
    }
}

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
        println!(
            "🐢🐢🐢 Previously reported task (id: {}) eventually completed 🐢🐢🐢",
            id
        );
    }
}

fn watching_thread() {
    loop {
        let (id, context) = TIMER.wait();

        // Using `println!` and not `tracing::*` to avoid circular dependencies because on
        // Android tracing uses `StateMonitor` which uses these mutexes.
        println!(
            "🐢🐢🐢 Task taking too long (id: {}) 🐢🐢🐢\n{}\n",
            id, context
        );
    }
}

use super::timer::{Id, Timer};
use once_cell::sync::Lazy;
use std::{
    backtrace::Backtrace,
    panic::Location,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tracing::Span;

/// Attach this to objects that are expected to be short-lived to be warned when they live longer
/// than expected.
pub struct ExpectShortLifetime {
    id: Id,
    start: Instant,
}

impl ExpectShortLifetime {
    #[track_caller]
    pub fn new(deadline: Duration) -> Self {
        Self::new_in(deadline, Location::caller())
    }

    pub fn new_in(deadline: Duration, location: &'static Location<'static>) -> Self {
        let context = Context::new(location, deadline);
        let id = schedule(deadline, context);

        Self {
            id,
            start: Instant::now(),
        }
    }
}

impl Drop for ExpectShortLifetime {
    fn drop(&mut self) {
        cancel(self.id, self.start);
    }
}

struct Context {
    deadline: Duration,
    span: Span,
    location: &'static Location<'static>,
    backtrace: Backtrace,
}

impl Context {
    fn new(location: &'static Location<'static>, deadline: Duration) -> Self {
        Self {
            deadline,
            span: Span::current(),
            location,
            backtrace: Backtrace::capture(),
        }
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

fn cancel(id: Id, start: Instant) {
    if TIMER.cancel(id).is_none() {
        tracing::warn!(
            "🐢🐢🐢 Previously reported task {} eventually completed in {:?} 🐢🐢🐢",
            id,
            start.elapsed(),
        );
    }
}

fn watching_thread() {
    loop {
        let (id, context) = TIMER.wait();

        let Context {
            deadline,
            span,
            location,
            backtrace,
        } = context;

        tracing::warn!(
            parent: span,
            "🐢🐢🐢 Task {} (started in {}) is taking longer than {:?} 🐢🐢🐢\n{}",
            id,
            location,
            deadline,
            backtrace,
        );
    }
}

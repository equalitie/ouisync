//! Replacements for tracing macros that add "ouisync-test" as target:

macro_rules! span {
    ($($tokens:tt)*) => {
        tracing::span!(target: "ouisync-test", $($tokens)*)
    }
}

macro_rules! info_span {
    ($($tokens:tt)*) => {
        span!(tracing::Level::INFO, $($tokens)*)
    }
}

macro_rules! event {
    ($($tokens:tt)*) => {
        tracing::event!(target: "ouisync-test", $($tokens)*)
    }
}

macro_rules! error {
    ($($tokens:tt)*) => {
        event!(tracing::Level::ERROR, $($tokens)*)
    }
}

macro_rules! warn {
    ($($tokens:tt)*) => {
        event!(tracing::Level::WARN, $($tokens)*)
    }
}

macro_rules! info {
    ($($tokens:tt)*) => {
        event!(tracing::Level::INFO, $($tokens)*)
    }
}

macro_rules! debug {
    ($($tokens:tt)*) => {
        event!(tracing::Level::DEBUG, $($tokens)*)
    }
}

macro_rules! trace {
    ($($tokens:tt)*) => {
        event!(tracing::Level::TRACE, $($tokens)*)
    }
}

macro_rules! event_enabled {
    ($($tokens:tt)*) => {
        tracing::event_enabled!(target: "ouisync-test", $($tokens)*)
    }
}

// Like `panic!`, but also logs the panic message with `tracing::error`. This is useful to locate
// the source of the panic in release mode where backtrace is typically useless.
macro_rules! tracing_panic {
    ($($tokens:tt)*) => {{
        let message = format_args!($($tokens)*);
        error!(location = %std::panic::Location::caller(), "{}", message);
        panic!("{}", message);
    }}
}

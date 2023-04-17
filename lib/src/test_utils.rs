use proptest::prelude::*;
use std::future::Future;

// proptest doesn't work with the `#[tokio::test]` macro yet
// (see https://github.com/AltSysrq/proptest/issues/179). As a workaround, create the runtime
// manually.
pub(crate) fn run<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(future)
}

pub(crate) fn rng_seed_strategy() -> impl Strategy<Value = u64> {
    any::<u64>().no_shrink()
}

pub(crate) fn init_log() {
    use tracing::metadata::LevelFilter;

    tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(LevelFilter::OFF.into())
                .from_env_lossy(),
        )
        .with_test_writer()
        .try_init()
        .ok();
}

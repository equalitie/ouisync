use proptest::prelude::*;

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
        .with_target(false)
        .with_test_writer()
        .try_init()
        .ok();
}

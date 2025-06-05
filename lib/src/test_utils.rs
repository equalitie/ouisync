use proptest::prelude::*;

pub(crate) fn rng_seed_strategy() -> impl Strategy<Value = u64> {
    any::<u64>().no_shrink()
}

pub(crate) fn init_log() {
    use tracing::metadata::LevelFilter;

    let result = tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(LevelFilter::OFF.into())
                .from_env_lossy(),
        )
        .with_target(false)
        .with_test_writer()
        .try_init();

    if result.is_ok() {
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let payload = panic_info.payload();
            let payload = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
                .unwrap_or("???");

            tracing::error!(
                "A panic occurred{}: {}",
                panic_info
                    .location()
                    .map(|l| format!(" at {}:{}:{}", l.file(), l.line(), l.column()))
                    .unwrap_or_default(),
                payload,
            );

            prev_hook(panic_info);
        }));
    }
}

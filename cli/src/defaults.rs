use std::{env, path::PathBuf};

pub(crate) const APP_NAME: &str = "ouisync";

/// Path to the config directory.
pub(crate) fn config_dir() -> PathBuf {
    env::var_os("OUISYNC_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .expect("config dir not defined")
                .join(APP_NAME)
        })
}

/// Path to the config directory.
pub(crate) fn store_dir() -> PathBuf {
    env::var_os("OUISYNC_STORE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_dir()
                .expect("data dir not defined")
                .join(APP_NAME)
        })
}

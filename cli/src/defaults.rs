use std::{env, path::PathBuf};

pub(crate) const APP_ID: &str = "org.equalitie.ouisync";

/// Path to the config directory.
pub(crate) fn config_dir() -> PathBuf {
    env::var_os("OUISYNC_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| base_dir().join("configs"))
}

/// Path to the repository store directory.
pub(crate) fn store_dir() -> PathBuf {
    env::var_os("OUISYNC_STORE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| base_dir().join("repositories"))
}

fn base_dir() -> PathBuf {
    dirs::data_dir().expect("data dir not defined").join(APP_ID)
}

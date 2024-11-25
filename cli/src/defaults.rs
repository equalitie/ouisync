use std::{env, path::PathBuf};

pub(crate) const APP_NAME: &str = "ouisync";

pub(crate) fn socket() -> PathBuf {
    env::var_os("OUISYNC_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(platform::socket)
}

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

pub(crate) fn mount_dir() -> PathBuf {
    env::var_os("OUISYNC_MOUNT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .expect("home dir not defined")
                .join(APP_NAME)
        })
}

mod platform {
    use super::*;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(super) fn socket() -> PathBuf {
        // FIXME: when running as root, we should use `/run`
        dirs::runtime_dir()
            .or_else(dirs::cache_dir)
            .expect("neither runtime dir nor cache dir defined")
            .join(APP_NAME)
            .with_extension("sock")
    }

    #[cfg(target_os = "windows")]
    pub(super) fn socket() -> PathBuf {
        format!(r"\\.\pipe\{APP_NAME}").into()
    }
}

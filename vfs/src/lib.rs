#[cfg(target_os = "linux")]
mod fuse;

#[cfg(target_os = "linux")]
pub use fuse::{mount, MountGuard, MultiRepoVFS};

#[cfg(target_os = "windows")]
mod dokan;

#[cfg(target_os = "windows")]
pub use crate::dokan::{
    multi_repo_mount::MultiRepoVFS,
    single_repo_mount::{mount, MountGuard},
};

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod dummy;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub use dummy::{mount, MountGuard, MultiRepoVFS};

#[cfg(test)]
mod tests;

use ouisync_lib::Repository;
use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};
use thiserror::Error;

pub trait MultiRepoMount {
    fn create(
        mount_point: impl AsRef<Path>,
    ) -> Pin<Box<dyn Future<Output = Result<Self, MountError>> + Send>>
    where
        Self: Sized;

    fn insert(&self, store_path: PathBuf, repo: Arc<Repository>) -> Result<(), io::Error>;

    fn remove(&self, store_path: &Path) -> Result<(), io::Error>;
}

#[derive(Debug, Error)]
pub enum MountError {
    #[error("Invalid mount point")]
    InvalidMountPoint,
    #[error("Mounting is not supported on this platform")]
    Unsupported,
    #[error("Can't install the backend driver")]
    DriverInstall,
    #[error("Backend error")]
    Backend(#[source] Box<dyn std::error::Error + Send + 'static>),
}

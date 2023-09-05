#[cfg(target_os = "windows")]
mod dokan;

#[cfg(target_os = "windows")]
pub use crate::dokan::{
    multi_repo_mount::MultiRepoVFS,
    single_repo_mount::{mount, MountGuard},
};

#[cfg(target_os = "linux")]
mod fuse;

#[cfg(any(target_os = "linux", target_os = "android"))]
mod dummy_multi_repo_mount;

#[cfg(test)]
mod tests;

#[cfg(target_os = "linux")]
pub use fuse::{mount, MountGuard};

#[cfg(any(target_os = "linux", target_os = "android"))]
pub use dummy_multi_repo_mount::MultiRepoVFS;

use thiserror::Error;

#[derive(Copy, Clone, Debug, Error)]
pub enum MountError {
    #[error("Failed to parse the mount point string")]
    FailedToParseMountPoint,
    #[error("Mounting is not yes supported on this Operating System")]
    UnsupportedOs,
    #[error("The driver responds that something is wrong")]
    Start,
    #[error("A general error")]
    General,
    #[error("Bad drive letter")]
    DriveLetter,
    #[error("Can't install the Dokan driver")]
    DriverInstall,
    #[error("Can't assign a drive letter or mount point")]
    Mount,
    #[error("The mount point is invalid")]
    MountPoint,
    #[error("The Dokan version that this wrapper is targeting is incompatible with the loaded Dokan library")]
    Version,
}

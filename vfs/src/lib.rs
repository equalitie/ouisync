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

#[cfg(target_os = "linux")]
pub use fuse::{mount, MountGuard};

#[cfg(any(target_os = "linux", target_os = "android"))]
pub use dummy_multi_repo_mount::MultiRepoVFS;

#[cfg(target_os = "windows")]
mod dokan;

#[cfg(target_os = "windows")]
pub use crate::dokan::{
    multi_repo_mount::MultiRepoVFS,
    single_repo_mount::{mount, MountGuard},
};

#[cfg(not(target_os = "windows"))]
mod fuse;

#[cfg(not(target_os = "windows"))]
pub use fuse::{mount, MountGuard};

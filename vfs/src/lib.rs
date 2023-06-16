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

#[repr(u16)]
#[derive(Copy, Clone, Debug)]
pub enum MountErrorCode {
    // Don't change the numeric values of these enums, they are exported through FFI.
    Success = 0,
    FailedToParseMountPoint = 1,
    UnsupportedOs = 2,
    Start = 3,
    General = 4,
    DriveLetter = 5,
    DriverInstall = 6,
    Mount = 7,
    MountPoint = 8,
    Version = 9,
}

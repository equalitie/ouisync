#[cfg(target_os = "windows")]
mod dokan_impl;

#[cfg(target_os = "windows")]
pub use dokan_impl::{mount, MountGuard};

#[cfg(not(target_os = "windows"))]
mod fuse;

#[cfg(not(target_os = "windows"))]
pub use fuse::{mount, MountGuard};

#[cfg(target_os = "windows")]
mod dokan_impl;

#[cfg(target_os = "windows")]
pub use dokan_impl::{mount, MountGuard};

#[cfg(not(target_os = "windows"))]
mod fuse;

#[cfg(not(target_os = "windows"))]
pub use fuse::{mount, MountGuard};

use camino::Utf8PathBuf;
use ouisync_lib::Repository;
use std::io;
use std::sync::Arc;

pub struct SessionMounter {}

impl SessionMounter {
    pub async fn mount() -> io::Result<Self> {
        todo!()
    }

    pub fn add_repo(&self, store_path: Utf8PathBuf, repo: Arc<Repository>) {
        todo!()
    }

    pub fn remove_repo(&self, store_path: Utf8PathBuf) {
        todo!()
    }
}

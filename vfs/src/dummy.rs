//! Dummy implementation that does nothing. Used on OSes that don't support mounting.

use crate::{MountError, MultiRepoMount};
use ouisync_lib::Repository;
use std::{
    future::{self, Future},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct MultiRepoVFS;

impl MultiRepoMount for MultiRepoVFS {
    fn create(
        _mount_root: impl AsRef<Path>,
    ) -> impl Future<Output = Result<Self, MountError>> + Send
    where
        Self: Sized,
    {
        future::ready(Err(MountError::Unsupported))
    }

    fn insert(&self, _repo_name: String, _repo: Arc<Repository>) -> Result<PathBuf, io::Error> {
        Err(io::ErrorKind::Unsupported.into())
    }

    fn remove(&self, _repo_name: &str) -> Result<(), io::Error> {
        Err(io::ErrorKind::Unsupported.into())
    }

    /// If the repo is mounted, returns its mount point. Otherwise return `None`.
    fn mount_point(&self, _repo_name: &str) -> Option<PathBuf> {
        None
    }

    fn mount_root(&self) -> &Path {
        Path::new("")
    }
}

pub struct MountGuard;

// To satisfy clippy: https://rust-lang.github.io/rust-clippy/master/index.html#drop_non_drop
impl Drop for MountGuard {
    fn drop(&mut self) {}
}

pub fn mount(
    _runtime_handle: tokio::runtime::Handle,
    _repository: Arc<Repository>,
    _mount_point: impl AsRef<Path>,
) -> Result<MountGuard, io::Error> {
    Err(io::ErrorKind::Unsupported.into())
}

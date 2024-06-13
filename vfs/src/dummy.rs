//! Dummy implementation that does nothing. Used on OSes that don't support mounting.

use crate::{MountError, MultiRepoMount};
use ouisync_lib::Repository;
use std::{
    future::{self, Future},
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

pub struct MultiRepoVFS;

impl MultiRepoMount for MultiRepoVFS {
    fn create(
        _mount_point: impl AsRef<Path>,
    ) -> Pin<Box<dyn Future<Output = Result<Self, MountError>> + Send>> {
        Box::pin(future::ready(Err(MountError::Unsupported)))
    }

    fn insert(&self, _store_path: PathBuf, _repo: Arc<Repository>) -> Result<(), io::Error> {
        Err(io::ErrorKind::Unsupported.into())
    }

    fn remove(&self, _store_path: &Path) -> Result<(), io::Error> {
        Err(io::ErrorKind::Unsupported.into())
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

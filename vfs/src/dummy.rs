use crate::{MountError, MultiRepoMount};
use camino::{Utf8Path, Utf8PathBuf};
use ouisync_lib::Repository;
use std::{
    future::{self, Future},
    path::Path,
    pin::Pin,
    sync::Arc,
};

/// Dummy implementation of `MultiRepoVFS` that does nothing. Used on OSes that don't support
/// mounting.
pub struct MultiRepoVFS;

impl MultiRepoMount for MultiRepoVFS {
    fn create(
        _runtime_handle: tokio::runtime::Handle,
        _mount_point: impl AsRef<Path>,
    ) -> Pin<Box<dyn Future<Output = Result<Self, MountError>>>> {
        Box::pin(future::ready(Err(MountError::Unsupported)))
    }

    fn insert(&self, _store_path: Utf8PathBuf, _repo: Arc<Repository>) -> Result<(), MountError> {
        Err(MountError::Unsupported)
    }

    fn remove(&self, _store_path: &Utf8Path) -> Result<(), MountError> {
        Err(MountError::Unsupported)
    }
}

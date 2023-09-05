use crate::{MountError, MultiRepoMount};
use ouisync_lib::Repository;
use std::{
    future::{self, Future},
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

/// Dummy implementation of `MultiRepoVFS` that does nothing. Used on OSes that don't support
/// mounting.
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

use crate::{MountError, MultiRepoMount};
use camino::{Utf8Path, Utf8PathBuf};
use ouisync_lib::Repository;
use std::{future::Future, path::Path, pin::Pin, sync::Arc};

// TODO: Implement this
pub struct MultiRepoVFS {}

impl MultiRepoMount for MultiRepoVFS {
    fn create(
        _runtime_handle: tokio::runtime::Handle,
        _mount_point: impl AsRef<Path>,
    ) -> Pin<Box<dyn Future<Output = Result<Self, MountError>> + Send>> {
        todo!()
    }

    fn insert(&self, _store_path: Utf8PathBuf, _repo: Arc<Repository>) -> Result<(), MountError> {
        todo!()
    }

    fn remove(&self, _store_path: &Utf8Path) -> Result<(), MountError> {
        todo!()
    }
}

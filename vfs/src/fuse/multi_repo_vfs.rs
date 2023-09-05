use crate::{MountError, MultiRepoMount};
use ouisync_lib::Repository;
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

// TODO: Implement this
pub struct MultiRepoVFS {}

impl MultiRepoMount for MultiRepoVFS {
    fn create(
        _mount_point: impl AsRef<Path>,
    ) -> Pin<Box<dyn Future<Output = Result<Self, MountError>> + Send>> {
        todo!()
    }

    fn insert(&self, _store_path: PathBuf, _repo: Arc<Repository>) -> Result<(), MountError> {
        todo!()
    }

    fn remove(&self, _store_path: &Path) -> Result<(), MountError> {
        todo!()
    }
}

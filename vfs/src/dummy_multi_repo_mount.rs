use super::MountError;
use camino::Utf8PathBuf;
use ouisync_lib::Repository;
use std::{io, path::Path, sync::Arc};

// TODO: Implement this
pub struct MultiRepoVFS {}

impl MultiRepoVFS {
    pub async fn mount(
        _runtime_handle: tokio::runtime::Handle,
        _mount_point: impl AsRef<Path>,
    ) -> Result<Self, MountError> {
        Err(MountError::UnsupportedOs)
    }

    pub fn add_repo(
        &self,
        _store_path: Utf8PathBuf,
        _repo: Arc<Repository>,
    ) -> Result<(), io::Error> {
        todo!()
    }

    pub fn remove_repo(&self, _store_path: Utf8PathBuf) {
        todo!()
    }
}

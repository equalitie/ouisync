use crate::{MountError, MultiRepoMount};
use ouisync_lib::Repository;
use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{mpsc, Arc},
};

pub fn mount(
    _runtime_handle: tokio::runtime::Handle,
    _repository: Arc<Repository>,
    _mount_point: impl AsRef<Path>,
) -> Result<MountGuard, io::Error> {
    // Mounting a single repo to a particular mount point is not supported on Darwin. Only multi-repo-mounting is supported
    // on these platform.
    return Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Single repository mounting is not supported on this platform",
    ));
}

pub struct MultiRepoVFS {}

impl MultiRepoMount for MultiRepoVFS {
    fn create(
        mount_point: impl AsRef<Path>,
    ) -> Pin<Box<dyn Future<Output = Result<Self, MountError>> + Send>> {
        todo!()
        //Box::pin(async move {
        //})
    }

    fn insert(&self, store_path: PathBuf, repo: Arc<Repository>) -> Result<(), io::Error> {
        todo!();
    }

    fn remove(&self, store_path: &Path) -> Result<(), io::Error> {
        todo!();
    }
}

pub struct MountGuard {}

impl Drop for MountGuard {
    fn drop(&mut self) {}
}

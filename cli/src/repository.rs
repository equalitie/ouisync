/*
use crate::{error::Error, options::Dirs, utils, DB_EXTENSION};
use camino::Utf8Path;
use ouisync_bridge::{config::ConfigStore, protocol::remote::v1, transport::RemoteClient};
use ouisync_lib::{Network, Registration, Repository};
use ouisync_vfs::MountGuard;
use state_monitor::StateMonitor;
use std::{
    borrow::{Borrow, Cow},
    collections::{btree_map::Entry, BTreeMap},
    ffi::OsStr,
    fmt, io, mem,
    ops::{Bound, Deref},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};
use thiserror::Error;
use tokio::{fs, runtime};
use tokio_rustls::rustls;
use tokio_stream::StreamExt;

// Config keys
pub(crate) const OPEN_ON_START: &str = "open_on_start";
const MOUNT_POINT_KEY: &str = "mount_point";

pub(crate) struct RepositoryHolder {
    pub repository: Arc<Repository>,
    pub registration: Registration,
    name: RepositoryName,
    mount: Mutex<Option<Mount>>,
}

impl RepositoryHolder {
    pub async fn new(repository: Repository, name: RepositoryName, network: &Network) -> Self {
        let repository = Arc::new(repository);
        let registration = network.register(repository.handle()).await;

        Self {
            repository,
            registration,
            name,
            mount: Mutex::new(None),
        }
    }

    pub fn name(&self) -> &RepositoryName {
        &self.name
    }

    pub fn is_mounted(&self) -> bool {
        self.mount.lock().unwrap().is_some()
    }

    /// Create a mirror of the repository on the given remote host.
    pub async fn mirror(&self, host: &str, config: Arc<rustls::ClientConfig>) -> Result<(), Error> {
        let secrets = self
            .repository
            .secrets()
            .into_write_secrets()
            .ok_or_else(|| Error::PermissionDenied)?;

        let client = RemoteClient::connect(host, config)
            .await
            .inspect_err(|error| tracing::error!(?error, host, "connection failed"))?;

        let proof = secrets.write_keys.sign(client.session_cookie().as_ref());
        let request = v1::Request::Create {
            repository_id: secrets.id,
            proof,
        };

        client
            .invoke(request)
            .await
            .inspect_err(|error| tracing::error!(?error, host, "request failed"))?;

        Ok(())
    }

    pub async fn close(&self) -> Result<(), Error> {
        self.unmount().await;

        self.repository
            .close()
            .await
            .inspect(|_| tracing::info!(name = %self.name, "Repository closed"))
            .inspect_err(
                |error| tracing::error!(name = %self.name, ?error, "Failed to close repository"),
            )?;

        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_path_sanity_check() {
        let store_dir = Path::new("/home/alice/ouisync/store");

        assert_eq!(
            store_path(store_dir, "foo"),
            Path::new("/home/alice/ouisync/store/foo.ouisyncdb")
        );

        assert_eq!(
            store_path(store_dir, "foo/bar"),
            Path::new("/home/alice/ouisync/store/foo/bar.ouisyncdb")
        );

        assert_eq!(
            store_path(store_dir, "foo/bar.baz"),
            Path::new("/home/alice/ouisync/store/foo/bar.baz.ouisyncdb")
        );
    }
}
*/

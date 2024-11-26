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

*/

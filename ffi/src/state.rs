use crate::{
    file::FileHolder,
    registry::{Handle, Registry},
    repository::RepositoryHolder,
};
use deadlock::BlockingMutex;
use once_cell::sync::OnceCell;
use ouisync_bridge::{config::ConfigStore, transport};
use ouisync_lib::network::Network;
use ouisync_vfs::{MountError, MultiRepoMount, MultiRepoVFS};
use scoped_task::ScopedJoinHandle;
use state_monitor::StateMonitor;
use std::{collections::BTreeSet, io, path::PathBuf, sync::Arc};

pub(crate) struct State {
    pub root_monitor: StateMonitor,
    pub repos_monitor: StateMonitor,
    pub config: ConfigStore,
    pub network: Network,
    repositories: Registry<RepositoryHolder>,
    pub files: Registry<FileHolder>,
    pub tasks: Registry<ScopedJoinHandle<()>>,
    pub storage_servers: BlockingMutex<BTreeSet<String>>,
    pub remote_client_config: OnceCell<Arc<rustls::ClientConfig>>,
    pub mounter: BlockingMutex<Option<MultiRepoVFS>>,
}

impl State {
    pub fn new(configs_path: PathBuf, root_monitor: StateMonitor) -> Self {
        let config = ConfigStore::new(configs_path);

        let network = Network::new(
            Some(config.dht_contacts_store()),
            root_monitor.make_child("Network"),
        );

        let repos_monitor = root_monitor.make_child("Repositories");

        Self {
            root_monitor,
            repos_monitor,
            config,
            network,
            repositories: Registry::new(),
            files: Registry::new(),
            tasks: Registry::new(),
            storage_servers: BlockingMutex::new(BTreeSet::new()),
            remote_client_config: OnceCell::new(),
            mounter: BlockingMutex::new(None),
        }
    }

    /// Cancel a notification subscription.
    pub fn unsubscribe(&self, handle: SubscriptionHandle) {
        self.tasks.remove(handle);
    }

    pub fn get_remote_client_config(&self) -> Result<Arc<rustls::ClientConfig>, io::Error> {
        self.remote_client_config
            .get_or_try_init(|| transport::make_client_config(&[]))
            .cloned()
    }

    pub fn insert_repository(&self, holder: RepositoryHolder) -> Handle<RepositoryHolder> {
        if let Some(mounter) = &*self.mounter.lock().unwrap() {
            if let Err(error) = mounter.insert(holder.store_path.clone(), holder.repository.clone())
            {
                tracing::error!(
                    "Failed to mount repository {:?}: {error:?}",
                    holder.store_path
                );
            }
        }

        self.repositories.insert(holder)
    }

    pub fn remove_repository(&self, handle: Handle<RepositoryHolder>) -> Option<RepositoryHolder> {
        let holder = self.repositories.remove(handle)?;

        if let Some(mounter) = &*self.mounter.lock().unwrap() {
            if let Err(error) = mounter.remove(&holder.store_path) {
                tracing::error!(
                    "Failed to unmount repository {:?}: {error:?}",
                    holder.store_path
                );
            }
        }

        Some(holder)
    }

    pub fn get_repository(&self, handle: Handle<RepositoryHolder>) -> Arc<RepositoryHolder> {
        self.repositories.get(handle)
    }

    pub fn get_all_repositories(&self) -> Vec<Arc<RepositoryHolder>> {
        self.repositories.get_all()
    }

    pub async fn mount_all(&self, mount_point: PathBuf) -> Result<(), MountError> {
        let mounter = MultiRepoVFS::create(mount_point).await.map_err(|error| {
            tracing::error!("Failed create mounter: {error:?}");
            error
        })?;

        for repo_holder in self.get_all_repositories() {
            if let Err(error) = mounter.insert(
                repo_holder.store_path.clone(),
                repo_holder.repository.clone(),
            ) {
                tracing::error!(
                    "Failed to mount repository {:?}: {error:?}",
                    repo_holder.store_path
                );
            }
        }

        *self.mounter.lock().unwrap() = Some(mounter);

        Ok(())
    }
}

pub(crate) type SubscriptionHandle = Handle<ScopedJoinHandle<()>>;

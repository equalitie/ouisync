use crate::{
    file::FileHolder,
    registry::{Handle, Registry},
    repository::RepositoryHolder,
};
use once_cell::sync::OnceCell;
use ouisync_bridge::{config::ConfigStore, error::Result, transport::ClientConfig};
use ouisync_lib::{
    deadlock::{BlockingMutex, BlockingRwLockReadGuard},
    network::Network,
    StateMonitor,
};
use ouisync_vfs::SessionMounter;
use scoped_task::ScopedJoinHandle;
use std::{collections::HashMap, path::PathBuf, sync::Arc};

pub(crate) struct State {
    pub root_monitor: StateMonitor,
    pub repos_monitor: StateMonitor,
    pub config: ConfigStore,
    pub network: Network,
    repositories: Registry<RepositoryHolder>,
    pub files: Registry<FileHolder>,
    pub tasks: Registry<ScopedJoinHandle<()>>,
    pub remote_client_config: OnceCell<ClientConfig>,
    pub mounter: BlockingMutex<Option<SessionMounter>>,
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
            remote_client_config: OnceCell::new(),
            mounter: BlockingMutex::new(None),
        }
    }

    /// Cancel a notification subscription.
    pub fn unsubscribe(&self, handle: SubscriptionHandle) {
        self.tasks.remove(handle);
    }

    pub fn get_remote_client_config(&self) -> Result<ClientConfig> {
        self.remote_client_config
            .get_or_try_init(|| ClientConfig::new(&[]))
            .cloned()
    }

    pub fn insert_repository(&self, holder: RepositoryHolder) -> Handle<RepositoryHolder> {
        if let Some(mounter) = &*self.mounter.lock().unwrap() {
            mounter.add_repo(holder.store_path.clone(), holder.repository.clone());
        }

        self.repositories.insert(holder)
    }

    pub fn remove_repository(&self, handle: Handle<RepositoryHolder>) -> Option<RepositoryHolder> {
        if let Some(mounter) = &*self.mounter.lock().unwrap() {
            mounter.remove_repo(holder.store_path.clone());
        }

        self.repositories.remove(handle)
    }

    pub fn get_repository(&self, handle: Handle<RepositoryHolder>) -> Arc<RepositoryHolder> {
        self.repositories.get(handle)
    }

    pub fn read_repositories(
        &self,
    ) -> BlockingRwLockReadGuard<HashMap<u64, Arc<RepositoryHolder>>> {
        self.repositories.read()
    }
}

pub(crate) type SubscriptionHandle = Handle<ScopedJoinHandle<()>>;

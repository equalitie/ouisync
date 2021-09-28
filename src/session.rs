use crate::{
    config, db,
    error::{Error, Result},
    network::{Network, NetworkOptions},
    repository::{Repository, RepositoryManager},
};
use std::net::SocketAddr;

// TODO: this is not needed, remove it.

/// Entry point to this library.
pub struct Session {
    repositories: RepositoryManager,
    network: Network,
}

impl Session {
    /// Creates a new session.
    pub async fn new(
        config_db_store: db::Store,
        enable_merger: bool,
        network_options: NetworkOptions,
    ) -> Result<Self> {
        let pool = config::open_db(config_db_store).await?;
        let repositories = RepositoryManager::load(pool, enable_merger).await?;

        let subscription = repositories.subscribe();
        let network = Network::new(subscription, network_options).await?;

        Ok(Self {
            repositories,
            network,
        })
    }

    pub fn repositories(&self) -> &RepositoryManager {
        &self.repositories
    }

    pub fn repositories_mut(&mut self) -> &mut RepositoryManager {
        &mut self.repositories
    }

    /// Opens a repository.
    ///
    /// NOTE: Currently only one repository is supported but in the future this function will take
    /// an argument to specify which repository to open.
    pub fn open_repository(&self) -> Repository {
        todo!()
        // Repository::new(self.index.clone(), self.cryptor.clone(), enable_merger)
    }

    /// Returns the local socket address the network listener is bound to.
    pub fn local_addr(&self) -> &SocketAddr {
        self.network.local_addr()
    }
}

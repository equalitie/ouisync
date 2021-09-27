use crate::{
    config,
    crypto::Cryptor,
    db,
    error::{Error, Result},
    index::Index,
    network::{Network, NetworkOptions},
    replica_id::ReplicaId,
    repository::Repository,
    this_replica,
};
use std::net::SocketAddr;

/// Entry point to this library.
pub struct Session {
    // TODO: cryptor should probably be per repository
    cryptor: Cryptor,
    index: Index,
    network: Network,
}

impl Session {
    /// Creates a new session.
    pub async fn new(
        config_db_store: db::Store,
        cryptor: Cryptor,
        network_options: NetworkOptions,
    ) -> Result<Self> {
        let pool = config::open_db(config_db_store).await?;

        let this_replica_id = this_replica::get_or_create_id(&pool).await?;
        let index = Index::load(pool.clone(), this_replica_id).await?;
        let network = Network::new(index.clone(), network_options)
            .await
            .map_err(Error::Network)?;

        Ok(Self {
            cryptor,
            index,
            network,
        })
    }

    /// Opens a repository.
    ///
    /// NOTE: Currently only one repository is supported but in the future this function will take
    /// an argument to specify which repository to open.
    pub fn open_repository(&self, enable_merger: bool) -> Repository {
        Repository::new(self.index.clone(), self.cryptor.clone(), enable_merger)
    }

    /// Returns the local socket address the network listener is bound to.
    pub fn local_addr(&self) -> &SocketAddr {
        self.network.local_addr()
    }

    pub fn this_replica_id(&self) -> &ReplicaId {
        self.index.this_replica_id()
    }
}

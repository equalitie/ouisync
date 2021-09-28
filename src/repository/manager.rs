use super::IndexMap;
use crate::{crypto::Cryptor, replica_id::ReplicaId};
use std::sync::Arc;
use tokio::sync::{RwLock, RwLockReadGuard};

pub struct RepositoryManager {
    index_map: Arc<RwLock<IndexMap>>,
    cryptor: Cryptor,
}

impl RepositoryManager {
    pub(crate) fn new(index_map: Arc<RwLock<IndexMap>>, cryptor: Cryptor) -> Self {
        Self { index_map, cryptor }
    }

    pub async fn read(&self) -> Reader<'_> {
        let index_map = self.index_map.read().await;

        Reader {
            index_map,
            cryptor: &self.cryptor,
        }
    }
}

pub struct Reader<'a> {
    index_map: RwLockReadGuard<'a, IndexMap>,
    cryptor: &'a Cryptor,
}

impl Reader<'_> {
    pub fn this_replica_id(&self) -> &ReplicaId {
        self.index_map.this_replica_id()
    }
}

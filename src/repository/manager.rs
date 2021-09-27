use crate::{crypto::Cryptor, db, replica_id::ReplicaId};

pub(crate) struct RepositoryManager {}

impl RepositoryManager {
    pub fn new(_pool: db::Pool, _this_replica_id: ReplicaId, _cryptor: Cryptor) -> Self {
        todo!()
    }
}

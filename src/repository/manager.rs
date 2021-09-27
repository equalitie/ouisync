use crate::{crypto::Cryptor, db};

pub(crate) struct RepositoryManager {}

impl RepositoryManager {
    pub fn new(_pool: db::Pool, _cryptor: Cryptor) -> Self {
        todo!()
    }
}

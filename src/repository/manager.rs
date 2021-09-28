use super::{name::RepositoryName, Repository};
use crate::{
    crypto::Cryptor,
    db,
    error::{Error, Result},
    index::Index,
    replica_id::ReplicaId,
    this_replica,
};
use futures_util::TryStreamExt;
use sqlx::Row;
use std::collections::BTreeMap;

pub struct RepositoryManager {
    pool: db::Pool,
    repositories: BTreeMap<RepositoryName, Repository>,
    this_replica_id: ReplicaId,
    enable_merger: bool,
}

impl RepositoryManager {
    pub async fn load(pool: db::Pool, enable_merger: bool) -> Result<Self> {
        let this_replica_id = this_replica::get_or_create_id(&pool).await?;

        let repositories = sqlx::query("SELECT name, path FROM repositories")
            .fetch(&pool)
            .err_into()
            .and_then(|row| async move {
                let name = row.get(0);
                let store = row.get(1);
                let pool = super::open_db(store).await?;
                let index = Index::load(pool, this_replica_id).await?;
                let repository = Repository::new(index, Cryptor::Null, enable_merger);

                Ok::<_, Error>((name, repository))
            })
            .try_collect()
            .await?;

        Ok(Self {
            pool,
            repositories,
            this_replica_id,
            enable_merger,
        })
    }

    /// Creates a new repository.
    pub async fn create(
        &mut self,
        _name: String,
        store: db::Store,
        cryptor: Cryptor,
    ) -> Result<()> {
        todo!()
    }

    /// Deletes an existing repository.
    pub async fn delete(&mut self, _name: &str) -> Result<()> {
        todo!()
    }

    /// Gets a repository with the specified name or `None` if no such repository exists.
    pub fn get(&self, name: &str) -> Option<&Repository> {
        self.repositories.get(name)
    }

    pub fn this_replica_id(&self) -> &ReplicaId {
        &self.this_replica_id
    }

    pub fn subscribe(&self) -> RepositorySubscription {
        RepositorySubscription {
            this_replica_id: self.this_replica_id,
            indices: self
                .repositories
                .iter()
                .map(|(name, repository)| (name.clone(), repository.index().clone()))
                .collect(),
        }
    }
}

pub struct RepositorySubscription {
    this_replica_id: ReplicaId,
    indices: BTreeMap<RepositoryName, Index>,
}

impl RepositorySubscription {
    pub(crate) fn this_replica_id(&self) -> &ReplicaId {
        &self.this_replica_id
    }

    pub(crate) fn current(&self) -> impl Iterator<Item = (&RepositoryName, &Index)> {
        self.indices.iter()
    }
}

/// Initialize the database schema for the repository manager.
pub(crate) async fn init(pool: &db::Pool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS repositories (
             name TEXT NOT NULL UNIQUE,
             path TEXT NOT NULL
         )",
    )
    .execute(pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}

/*
/// Map of repository indices.
pub(crate) struct IndexMap {
    values: HashMap<RepositoryId, Value>,
    ids: HashMap<RepositoryName, RepositoryId>,
}

impl IndexMap {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
            ids: HashMap::new(),
        }
    }

    /// Insert an index into this map. Returns `EntryExist` error if either `id` or `name` already
    /// exists in the map.
    pub fn insert(&mut self, id: RepositoryId, name: RepositoryName, index: Index) -> Result<()> {
        match (self.values.entry(id), self.ids.entry(name.clone())) {
            (hash_map::Entry::Vacant(value_entry), hash_map::Entry::Vacant(id_entry)) => {
                value_entry.insert(Value { index, name });
                id_entry.insert(id);
                Ok(())
            }
            _ => {
                // TODO: should we have a separate error variant (e.g. `RepositoryExists`) for this?
                Err(Error::EntryExists)
            }
        }
    }

    // pub async fn create(
    //     &mut self,
    //     name: RepositoryName,
    //     store: db::Store,
    // ) -> Result<(RepositoryId, &Index)> {
    //     if self.ids.contains_key(&name) {
    //         // TODO: should we have a separate error variant (e.g. `RepositoryExists`) for this?
    //         return Err(Error::EntryExists);
    //     }

    //     let mut tx = self.pool.begin().await?;
    //     let query_result = sqlx::query("INSERT INTO repositories (name, db_path) VALUES (?, ?)")
    //         .bind(&name)
    //         .bind(&store)
    //         .execute(&mut tx)
    //         .await?;

    //     let id = RepositoryId(query_result.last_insert_rowid() as _);

    //     let repo_pool = super::open_db(store).await?;
    //     let index = Index::load(repo_pool, self.this_replica_id).await?;
    //     let value = Value {
    //         index,
    //         name: name.clone(),
    //     };

    //     let index = &self
    //         .values
    //         .entry(id)
    //         .and_modify(|_| unreachable!())
    //         .or_insert(value)
    //         .index;

    //     self.ids.insert(name, id);

    //     tx.commit().await?;

    //     Ok((id, index))
    // }

    // pub async fn destroy(&mut self, id: RepositoryId) -> Result<()> {
    //     let mut tx = self.pool.begin().await?;

    //     let store = sqlx::query("SELECT db_path FROM repositories WHERE id = ?")
    //         .bind(id)
    //         .fetch_one(&mut tx)
    //         .await?
    //         .get(0);

    //     sqlx::query("DELETE FROM repositories WHERE id = ? LIMIT 1")
    //         .bind(id)
    //         .execute(&mut tx)
    //         .await?;

    //     let value = self.values.remove(&id).ok_or(Error::EntryNotFound)?;
    //     self.ids.remove(&value.name);

    //     value.index.pool.close().await;

    //     db::delete(store).await?;

    //     tx.commit().await?;

    //     Ok(())
    // }

    pub fn get(&self, id: RepositoryId) -> Option<&Index> {
        self.values.get(&id).map(|value| &value.index)
    }

    pub fn lookup(&self, name: &str) -> Option<(RepositoryId, &RepositoryName, &Index)> {
        let id = self.ids.get(name)?;
        let value = &self.values.get(id)?;

        Some((*id, &value.name, &value.index))
    }

    pub fn iter(&self) -> impl Iterator<Item = (RepositoryId, &RepositoryName, &Index)> {
        self.ids.iter().filter_map(move |(name, id)| {
            self.values.get(id).map(|value| (*id, name, &value.index))
        })
    }
}

struct Value {
    index: Index,
    name: RepositoryName,
}

*/

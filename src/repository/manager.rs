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
use std::collections::{btree_map::Entry, BTreeMap};

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
        name: String,
        store: db::Store,
        _cryptor: Cryptor, // TODO: cryptor is currently ignored
    ) -> Result<()> {
        let name = RepositoryName::from(name);
        let mut tx = self.pool.begin().await?;

        sqlx::query("INSERT INTO repositories (name, path) VALUES (?, ?)")
            .bind(&name)
            .bind(&store)
            .execute(&mut tx)
            .await?;

        let pool = super::open_db(store).await?;
        let index = Index::load(pool, self.this_replica_id).await?;
        let repository = Repository::new(index, Cryptor::Null, self.enable_merger);

        match self.repositories.entry(name) {
            Entry::Vacant(entry) => {
                entry.insert(repository);
            }
            Entry::Occupied(_) => {
                // TODO: should we have a separate error variant (e.g. `RepositoryExists`) for this?
                return Err(Error::EntryExists);
            }
        }

        tx.commit().await?;

        Ok(())
    }

    /// Deletes an existing repository.
    pub async fn delete(&mut self, name: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        let row = sqlx::query("SELECT id, path FROM repositories WHERE name = ?")
            .bind(name)
            .fetch_one(&mut tx)
            .await?;

        let id: u32 = row.get(0);
        let store = row.get(1);

        sqlx::query("DELETE FROM repositories WHERE id = ? LIMIT 1")
            .bind(id)
            .execute(&mut tx)
            .await?;

        let repository = self.repositories.remove(name).ok_or(Error::EntryNotFound)?;

        // TODO: send delete notification

        repository.index().pool.close().await;
        db::delete(store).await?;

        tx.commit().await?;

        Ok(())
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

use super::meta::{RepositoryId, RepositoryName};
use crate::{
    db,
    error::{Error, Result},
    index::Index,
    replica_id::ReplicaId,
    this_replica,
};
use futures_util::{future, TryStreamExt};
use sqlx::Row;
use std::collections::{hash_map, HashMap};

/// Map of repository indices.
pub(crate) struct IndexMap {
    pool: db::Pool,
    this_replica_id: ReplicaId,
    values: HashMap<RepositoryId, Value>,
    ids: HashMap<RepositoryName, RepositoryId>,
}

impl IndexMap {
    pub async fn new(pool: db::Pool) -> Result<Self> {
        let this_replica_id = this_replica::get_or_create_id(&pool).await?;

        let mut values = HashMap::new();
        let mut ids = HashMap::new();

        sqlx::query("SELECT id, name, db_path FROM repositories")
            .fetch(&pool)
            .err_into()
            .and_then(|row| async move {
                let id: RepositoryId = row.get(0);
                let name: RepositoryName = row.get(1);

                let store = row.get(2);
                let repo_pool = super::open_db(store).await?;
                let index = Index::load(repo_pool, this_replica_id).await?;

                Ok::<_, Error>((id, name, index))
            })
            .try_for_each(|(id, name, index)| {
                values.insert(
                    id,
                    Value {
                        index,
                        name: name.clone(),
                    },
                );
                ids.insert(name, id);

                future::ready(Ok(()))
            })
            .await?;

        Ok(Self {
            pool,
            this_replica_id,
            values,
            ids,
        })
    }

    pub async fn create(
        &mut self,
        name: RepositoryName,
        store: db::Store,
    ) -> Result<(RepositoryId, &Index)> {
        if self.ids.contains_key(&name) {
            // TODO: should we have a separate error variant (e.g. `RepositoryExists`) for this?
            return Err(Error::EntryExists);
        }

        let mut tx = self.pool.begin().await?;
        let query_result = sqlx::query("INSERT INTO repositories (name, db_path) VALUES (?, ?)")
            .bind(&name)
            .bind(&store)
            .execute(&mut tx)
            .await?;

        let id = RepositoryId(query_result.last_insert_rowid() as _);

        let repo_pool = super::open_db(store).await?;
        let index = Index::load(repo_pool, self.this_replica_id).await?;
        let value = Value {
            index,
            name: name.clone(),
        };

        let index = &self
            .values
            .entry(id)
            .and_modify(|_| unreachable!())
            .or_insert(value)
            .index;

        self.ids.insert(name, id);

        tx.commit().await?;

        Ok((id, index))
    }

    pub async fn destroy(&mut self, id: RepositoryId) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        let store = sqlx::query("SELECT db_path FROM repositories WHERE id = ?")
            .bind(id)
            .fetch_one(&mut tx)
            .await?
            .get(0);

        sqlx::query("DELETE FROM repositories WHERE id = ? LIMIT 1")
            .bind(id)
            .execute(&mut tx)
            .await?;

        let value = self.values.remove(&id).ok_or(Error::EntryNotFound)?;
        self.ids.remove(&value.name);

        value.index.pool.close().await;

        db::delete(store).await?;

        tx.commit().await?;

        Ok(())
    }

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

    pub fn this_replica_id(&self) -> &ReplicaId {
        &self.this_replica_id
    }
}

struct Value {
    index: Index,
    name: RepositoryName,
}

/// Initialize the database schema for the index map.
pub(crate) async fn init(pool: &db::Pool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS repositories (
             id      INTEGER PRIMARY KEY,
             name    TEXT NOT NULL UNIQUE,
             db_path TEXT NOT NULL
         )",
    )
    .execute(pool)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}

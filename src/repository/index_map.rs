use super::meta::{RepositoryId, RepositoryName};
use crate::{
    db,
    error::{Error, Result},
    index::Index,
    replica_id::ReplicaId,
};
use futures_util::{future, TryStreamExt};
use sqlx::Row;
use std::collections::{hash_map, HashMap};

/// Map of repository indices.
pub(crate) struct IndexMap {
    main_pool: db::Pool,
    this_replica_id: ReplicaId,
    values: HashMap<RepositoryId, Value>,
    ids: HashMap<RepositoryName, RepositoryId>,
}

impl IndexMap {
    pub async fn new(main_pool: db::Pool, this_replica_id: ReplicaId) -> Result<Self> {
        let mut values = HashMap::new();
        let mut ids = HashMap::new();

        sqlx::query("SELECT rowid, name, db_path FROM repositories")
            .fetch(&main_pool)
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
            main_pool,
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

        let mut tx = self.main_pool.begin().await?;
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
        let mut tx = self.main_pool.begin().await?;

        let store = sqlx::query("SELECT db_path FROM repositories WHERE rowid = ?")
            .bind(id)
            .fetch_one(&mut tx)
            .await?
            .get(0);

        sqlx::query("DELETE FROM repositories WHERE rowid = ? LIMIT 1")
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

    pub fn iter(&self) -> Iter {
        Iter {
            ids: self.ids.iter(),
            values: &self.values,
        }
    }
}

pub(crate) struct Iter<'a> {
    ids: hash_map::Iter<'a, RepositoryName, RepositoryId>,
    values: &'a HashMap<RepositoryId, Value>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = (RepositoryId, &'a RepositoryName, &'a Index);

    fn next(&mut self) -> Option<Self::Item> {
        let (name, id) = self.ids.next()?;
        let index = &self.values.get(id)?.index;

        Some((*id, name, index))
    }
}

impl<'a> IntoIterator for &'a IndexMap {
    type Item = <Self::IntoIter as Iterator>::Item;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

struct Value {
    index: Index,
    name: RepositoryName,
}

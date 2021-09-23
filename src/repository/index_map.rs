use super::manager::RepositoryId;
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
    indices: HashMap<RepositoryId, Index>,
    ids: HashMap<String, RepositoryId>,
}

impl IndexMap {
    pub async fn new(main_pool: db::Pool, this_replica_id: ReplicaId) -> Result<Self> {
        let mut indices = HashMap::new();
        let mut ids = HashMap::new();

        sqlx::query("SELECT rowid, name, db_path FROM repositories")
            .fetch(&main_pool)
            .err_into()
            .and_then(|row| async move {
                let store = row.get(2);
                let pool = db::init(store).await?;
                let index = Index::load(pool, this_replica_id).await?;

                Ok::<_, Error>((row.get(0), row.get(1), index))
            })
            .try_for_each(|(id, name, index)| {
                indices.insert(id, index);
                ids.insert(name, id);

                future::ready(Ok(()))
            })
            .await?;

        Ok(Self {
            main_pool,
            this_replica_id,
            indices,
            ids,
        })
    }

    pub async fn create(
        &mut self,
        name: String,
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

        let pool = db::init(store).await?;
        let index = Index::load(pool, self.this_replica_id).await?;

        let index = self
            .indices
            .entry(id)
            .and_modify(|_| unreachable!())
            .or_insert(index);

        self.ids.insert(name, id);

        tx.commit().await?;

        Ok((id, index))
    }

    pub async fn destroy(&mut self, id: RepositoryId) -> Result<()> {
        todo!()
    }

    pub fn get(&self, id: RepositoryId) -> Option<&Index> {
        self.indices.get(&id)
    }

    pub fn lookup(&self, name: &str) -> Option<(RepositoryId, &Index)> {
        let id = self.ids.get(name)?;
        let index = self.indices.get(id)?;

        Some((*id, index))
    }

    pub fn iter(&self) -> Iter {
        Iter {
            ids: self.ids.iter(),
            indices: &self.indices,
        }
    }
}

pub(crate) struct Iter<'a> {
    ids: hash_map::Iter<'a, String, RepositoryId>,
    indices: &'a HashMap<RepositoryId, Index>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = (RepositoryId, &'a str, &'a Index);

    fn next(&mut self) -> Option<Self::Item> {
        let (name, id) = self.ids.next()?;
        let index = self.indices.get(id)?;

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

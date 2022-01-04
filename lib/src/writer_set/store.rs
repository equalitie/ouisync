#![allow(dead_code)]

use super::{error::Error as WsError, Entry, WriterSet};
use crate::{
    crypto::{sign::Keypair, Hash},
    db,
    error::{Error, Result},
};
use sqlx::Row;
use std::collections::{HashMap, HashSet};

/// Persistence layer for the WriterSet structure.
pub struct Store {
    pool: db::Pool,
    writer_set: WriterSet,
}

impl Store {
    /// Create the database tables with a newly generated WriterSet.
    pub async fn create_new(pool: &db::Pool) -> Result<(Store, Keypair)> {
        let mut tx = pool.begin().await?;

        create_tables(&mut tx).await?;

        if load_origin_hash(&mut tx).await?.is_some() {
            return Err(WsError::WriterSetIsAlreadySetUp.into());
        }

        let (writer_set, keypair) = WriterSet::generate();

        write_origin_hash(&writer_set.origin, &mut tx).await?;

        for entry in writer_set.entries() {
            insert_valid_entry(entry, &mut tx).await?;
        }

        let db = Store {
            pool: pool.clone(),
            writer_set,
        };

        tx.commit().await?;

        Ok((db, keypair))
    }

    /// Create the database tables and initialize it with the origin entry.
    pub async fn create_existing(origin_entry: &Entry, pool: db::Pool) -> Result<Store> {
        let mut tx = pool.begin().await?;

        if load_origin_hash(&mut tx).await?.is_some() {
            return Err(WsError::WriterSetIsAlreadySetUp.into());
        }

        if let Some(writer_set) = WriterSet::new_from_existing(origin_entry) {
            let mut tx = pool.begin().await?;

            create_tables(&mut tx).await?;
            write_origin_hash(&writer_set.origin, &mut tx).await?;
            insert_valid_entry(origin_entry, &mut tx).await?;

            tx.commit().await?;

            Ok(Store { pool, writer_set })
        } else {
            Err(WsError::InvalidOriginEntry.into())
        }
    }

    /// Load existing database.
    pub async fn load(pool: db::Pool) -> Result<Store> {
        // Only reading here, so no need to commit the transaction.
        let mut tx = pool.begin().await?;

        let origin_hash = match load_origin_hash(&mut tx).await? {
            Some(origin_hash) => origin_hash,
            None => {
                return Err(WsError::OriginNotFound.into());
            }
        };

        let mut entries = load_entries(&mut tx).await?;

        let origin = entries
            .remove(&origin_hash)
            .ok_or(WsError::OriginNotFound)?;

        let mut writer_set =
            WriterSet::new_from_existing(&origin).ok_or(WsError::InvalidOriginEntry)?;

        // TODO: This has complexity O(N^2) and can probably be optimized.
        while !entries.is_empty() {
            let mut used = HashSet::new();

            for entry in entries.values().cloned() {
                if let Some(entry) = writer_set.prepare_entry(entry) {
                    used.insert(*entry.hash());
                    entry.insert();
                }
            }

            if used.is_empty() {
                log::warn!("writer_set: Found unusable entries");
                break;
            }

            for e in used {
                entries.remove(&e);
            }
        }

        Ok(Self { pool, writer_set })
    }

    pub async fn try_add_entry(&mut self, entry: Entry) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        match self.writer_set.prepare_entry(entry) {
            Some(entry) => {
                insert_valid_entry(&entry.entry, &mut tx).await?;
                tx.commit().await?;
                entry.insert();
                Ok(())
            }
            None => Err(WsError::InvalidEntry.into()),
        }
    }

    pub fn origin_entry(&self) -> &Entry {
        self.writer_set.origin_entry()
    }
}

/// Insert an entry which is known by this application to have valid entry.hash and
/// entry.signature.
async fn insert_valid_entry(entry: &Entry, tx: &mut db::Transaction<'_>) -> Result<()> {
    sqlx::query(
        "INSERT INTO writer_set_entries(writer, added_by, nonce, hash, signature)
            VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&entry.writer)
    .bind(&entry.added_by)
    .bind(&entry.nonce[..])
    .bind(&entry.hash)
    .bind(&entry.signature)
    .execute(tx)
    .await?;

    Ok(())
}

async fn load_entries(tx: &mut db::Transaction<'_>) -> Result<HashMap<Hash, Entry>> {
    use std::cell::Cell;

    sqlx::query("SELECT writer, added_by, nonce, hash, signature FROM writer_set_entries")
        .fetch_all(tx)
        .await?
        .into_iter()
        .map(|row| {
            let nonce: &[u8] = row.get(2);

            let e = Entry {
                writer: row.get(0),
                added_by: row.get(1),
                nonce: nonce.try_into()?,
                hash: row.get(3),
                signature: row.get(4),
                has_valid_hash: Cell::new(None),
                has_valid_signature: Cell::new(None),
            };

            Ok((e.hash, e))
        })
        .collect()
}

async fn load_origin_hash(tx: &mut db::Transaction<'_>) -> Result<Option<Hash>> {
    let h = sqlx::query("SELECT origin_hash FROM writer_set_origin")
        .fetch_optional(tx)
        .await?
        .map(|row| row.get(0));

    Ok(h)
}

async fn write_origin_hash(origin_hash: &Hash, tx: &mut db::Transaction<'_>) -> Result<()> {
    sqlx::query("INSERT INTO writer_set_origin(origin_hash) VALUES (?)")
        .bind(origin_hash)
        .execute(tx)
        .await?;

    Ok(())
}

async fn create_tables(tx: &mut db::Transaction<'_>) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS writer_set_entries (
             writer    BLOB NOT NULL,
             added_by  BLOB NOT NULL,
             nonce     BLOB NOT NULL,
             hash      BLOB NOT NULL,
             signature BLOB NOT NULL,

             UNIQUE(writer),
             UNIQUE(added_by, nonce)
         );",
    )
    .execute(&mut (*tx))
    .await
    .map_err(Error::CreateDbSchema)?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS writer_set_origin (
             -- Single row entry, represent the hash of the first writer which is 'self signed' in
             -- the writer_set_entries table.
             origin_hash BLOB NOT NULL
         );",
    )
    .execute(tx)
    .await
    .map_err(Error::CreateDbSchema)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{crypto::sign::Keypair, db};

    #[tokio::test(flavor = "multi_thread")]
    async fn sanity() {
        let pool = db::open_or_create(&db::Store::Memory).await.unwrap();
        let (mut store, alice) = Store::create_new(&pool).await.unwrap();

        let bob = Keypair::random();
        let bob_nonce = rand::random();

        let entry = Entry::new(&bob.public, &alice, bob_nonce);

        assert!(store.try_add_entry(entry).await.is_ok());

        let malory = Keypair::random();
        let malory_nonce = rand::random();

        let carol = Keypair::random();
        let carol_nonce = rand::random();

        assert!(store
            .try_add_entry(Entry::new(&malory.public, &malory, malory_nonce))
            .await
            .is_err());
        assert!(store
            .try_add_entry(Entry::new(&carol.public, &malory, carol_nonce))
            .await
            .is_err());

        let mut store = Store::load(pool).await.unwrap();

        assert!(store
            .try_add_entry(Entry::new(&carol.public, &alice, carol_nonce))
            .await
            .is_ok());
        assert!(store
            .try_add_entry(Entry::new(&alice.public, &alice, rand::random()))
            .await
            .is_err());
    }
}

use super::{error::Error as WsError, Entry, WriterSet};
use crate::{
    crypto::{sign::Keypair, Hash},
    db,
    error::{Error, Result},
};
use sqlx::Row;
use std::collections::{HashMap, HashSet};

/// Persistence layer for the WriterSet structure.
pub struct Db {
    pool: db::Pool,
    writer_set: WriterSet,
}

impl Db {
    /// Create the database tables with a newly generated WriterSet.
    pub async fn create_new(pool: &db::Pool) -> Result<(Db, Keypair)> {
        let mut tx = pool.begin().await?;

        create_tables(&mut tx).await?;

        if load_origin_hash(&mut tx).await?.is_some() {
            return Err(WsError::WriterSetIsAlreadySetUp.into());
        }

        let (writer_set, keypair) = WriterSet::generate();

        write_origin_hash(&writer_set.origin, &mut tx).await?;

        for entry in writer_set.entries() {
            insert_valid_entry(&entry, &mut tx).await?;
        }

        let db = Db {
            pool: pool.clone(),
            writer_set,
        };

        tx.commit().await?;

        Ok((db, keypair))
    }

    /// Create the database tables and initialize it with the origin entry.
    pub async fn create_existing(origin_entry: &Entry, pool: db::Pool) -> Result<Db> {
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

            Ok(Db { pool, writer_set })
        } else {
            Err(WsError::InvalidOriginEntry.into())
        }
    }

    /// Load existing database.
    pub async fn load(pool: db::Pool) -> Result<Db> {
        // Only reading here, no no need to commit the transaction.
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

    pub fn try_add_entry(&mut self, entry: Entry) -> bool {
        match self.writer_set.prepare_entry(entry) {
            Some(entry) => {
                entry.insert();
                true
            }
            None => false,
        }
    }
}

/// Insert an entry which is known by this application to have valid entry.hash and
/// entry.signature.
async fn insert_valid_entry(entry: &Entry, tx: &mut db::Transaction<'_>) -> Result<()> {
    let e = sqlx::query(
        "INSERT INTO writer_set_entries(writer, added_by, hash, signature)
            VALUES (?, ?, ?, ?)",
    )
    .bind(&entry.writer)
    .bind(&entry.added_by)
    .bind(&entry.hash)
    .bind(&entry.signature)
    .execute(tx)
    .await;
    e?;

    Ok(())
}

async fn load_entries(tx: &mut db::Transaction<'_>) -> Result<HashMap<Hash, Entry>> {
    use std::cell::Cell;

    let entries =
        sqlx::query("SELECT writer,added_by,hash,signature FROM writer_set_entries")
            .fetch_all(tx)
            .await?
            .into_iter()
            .map(|row| {
                let e = Entry {
                    writer: row.get(0),
                    added_by: row.get(1),
                    hash: row.get(2),
                    signature: row.get(3),
                    has_valid_hash: Cell::new(None),
                    has_valid_signature: Cell::new(None),
                };
                (e.hash, e)
            })
            .collect();

    Ok(entries)
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
             writer BLOB NOT NULL,
             added_by BLOB NOT NULL,
             hash BLOB NOT NULL,
             signature BLOB NOT NULL
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
    use crate::{
        crypto::sign::Keypair,
        db,
    };

    #[tokio::test(flavor = "multi_thread")]
    async fn sanity() {
        let pool = db::open(&db::Store::Memory).await.unwrap();
        let (mut store, alice) = Db::create_new(&pool).await.unwrap();

        let bob = Keypair::generate();

        let entry = Entry::new(&bob.public, &alice);

        assert!(store.try_add_entry(entry));

        let malory = Keypair::generate();
        let carol = Keypair::generate();

        assert!(!store.try_add_entry(Entry::new(&malory.public, &malory)));
        assert!(!store.try_add_entry(Entry::new(&carol.public, &malory)));
    }
}

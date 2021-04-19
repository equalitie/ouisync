mod crypto;
mod error;
mod repository;

pub use self::{error::Error, repository::Repository};

use futures_util::stream::TryStreamExt;
use sqlx::{Row, SqlitePool};

/// This function can be called from other languages via FFI
#[no_mangle]
pub extern "C" fn hello_ffi() {
    println!("Hello world")
}

/// Async stuff
pub async fn sql_example() -> Result<(), Error> {
    let pool = SqlitePool::connect(":memory:").await?;

    let _ = sqlx::query(
        "CREATE TABLE blocks (
             id        BLOB PRIMARY KEY,
             version   BLOB NOT NULL,
             child_tag BLOB
         )",
    )
    .execute(&pool)
    .await?;

    let query = "INSERT INTO blocks (id, version, child_tag) VALUES (?, ?, ?)";
    sqlx::query(query)
        .bind("0001")
        .bind("0000")
        .bind("abcd")
        .execute(&pool)
        .await?;
    sqlx::query(query)
        .bind("0002")
        .bind("0000")
        .bind("efgh")
        .execute(&pool)
        .await?;
    sqlx::query(query)
        .bind("0003")
        .bind("0001")
        .bind("ijkl")
        .execute(&pool)
        .await?;

    let mut rows = sqlx::query("SELECT id, version, child_tag FROM blocks").fetch(&pool);

    while let Some(row) = rows.try_next().await? {
        let id: &str = row.get(0);
        let version: &str = row.get(1);
        let child_tag: &str = row.get(2);

        println!("id={} version={} child_tag={}", id, version, child_tag);
    }

    Ok(())
}

mod crypto;
mod error;
mod repository;

pub use self::{error::Error, repository::Repository};

use rusqlite::{params, Connection};
use tokio::task;

/// This function can be called from other languages via FFI
#[no_mangle]
pub extern "C" fn hello_ffi() {
    println!("Hello world")
}

/// Async stuff
pub async fn sql_example() -> Result<(), Error> {
    task::block_in_place(|| {
        let conn = Connection::open_in_memory()?;

        conn.execute(
            "CREATE TABLE blocks (
             id        BLOB PRIMARY KEY,
             version   BLOB NOT NULL,
             child_tag BLOB)",
            [],
        )?;

        let mut stmt =
            conn.prepare("INSERT INTO blocks (id, version, child_tag) VALUES (?1, ?2, ?3)")?;
        stmt.execute(params!["0001", "0000", "abcd"])?;
        stmt.execute(params!["0002", "0000", "efgh"])?;
        stmt.execute(params!["0003", "0001", "ijkl"])?;

        let mut stmt = conn.prepare("SELECT id, version, child_tag FROM blocks")?;
        let mut rows = stmt.query([])?;

        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let version: String = row.get(1)?;
            let child_tag: String = row.get(2)?;

            println!("id={} version={} child_tag={}", id, version, child_tag);
        }

        Ok(())
    })
}

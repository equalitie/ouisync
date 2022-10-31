use super::{get_pragma, set_pragma, Connection, Error};

/// Apply all pending migrations.
pub(super) async fn run(conn: &mut Connection) -> Result<(), Error> {
    apply(conn, 1, include_str!("v1.sql")).await?;
    apply(conn, 2, include_str!("v2.sql")).await?;
    apply(conn, 3, include_str!("v3.sql")).await?;

    Ok(())
}

async fn apply(conn: &mut Connection, dst_version: u32, sql: &str) -> Result<(), Error> {
    let mut tx = conn.begin().await?;

    let src_version = get_version(&mut tx).await?;
    if src_version >= dst_version {
        return Ok(());
    }

    assert_eq!(
        dst_version,
        src_version + 1,
        "migrations must be applied in order"
    );

    sqlx::query(sql).execute(&mut *tx).await?;
    set_version(&mut tx, dst_version).await?;

    tx.commit().await?;

    Ok(())
}

async fn get_version(conn: &mut Connection) -> Result<u32, Error> {
    get_pragma(conn, "user_version").await
}

async fn set_version(conn: &mut Connection, value: u32) -> Result<(), Error> {
    set_pragma(conn, "user_version", value).await
}

use super::{get_pragma, set_pragma, Connection, Error, Pool};
use include_dir::{include_dir, Dir, File};
use once_cell::sync::Lazy;

/// Latest schema version
pub static SCHEMA_VERSION: Lazy<u32> = Lazy::new(|| {
    MIGRATIONS
        .files()
        .filter_map(get_migration)
        .map(|(version, _)| version)
        .max()
        .unwrap_or(0)
});

/// Apply all pending migrations.
pub(super) async fn run(pool: &Pool) -> Result<(), Error> {
    let mut migrations: Vec<_> = MIGRATIONS.files().filter_map(get_migration).collect();
    migrations.sort_by_key(|(version, _)| *version);

    for (version, sql) in migrations {
        apply(pool, version, sql).await?;
    }

    Ok(())
}

static MIGRATIONS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/src/db/migrations");

fn get_migration<'a>(file: &'a File<'_>) -> Option<(u32, &'a str)> {
    if !file
        .path()
        .extension()
        .map(|ext| ext == "sql")
        .unwrap_or(false)
    {
        return None;
    }

    let stem = file.path().file_stem()?.to_str()?;

    if !stem.starts_with('v') {
        return None;
    }
    let version: u32 = stem[1..].parse().ok()?;
    let sql = file.contents_utf8()?;

    Some((version, sql))
}

async fn apply(pool: &Pool, dst_version: u32, sql: &str) -> Result<(), Error> {
    let mut tx = pool.begin_write().await?;

    let src_version = get_version(&mut tx).await?;
    if src_version >= dst_version {
        return Ok(());
    }

    assert_eq!(
        dst_version,
        src_version + 1,
        "migrations must be applied in order"
    );

    sqlx::query(sql).execute(&mut tx).await?;
    set_version(&mut tx, dst_version).await?;

    tx.commit().await?;

    Ok(())
}

/// Gets the current schema version of the database.
async fn get_version(conn: &mut Connection) -> Result<u32, Error> {
    get_pragma(conn, "user_version").await
}

/// Sets the current schema version of the database.
async fn set_version(conn: &mut Connection, value: u32) -> Result<(), Error> {
    set_pragma(conn, "user_version", value).await
}

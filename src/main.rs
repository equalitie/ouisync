mod options;
mod virtual_filesystem;

use self::options::Options;
use anyhow::{Context, Result};
use ouisync::BlockStore;
use sqlx::{sqlite::SqliteConnectOptions, SqlitePool};
use structopt::StructOpt;
use tokio::{fs, signal};

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::from_args();

    env_logger::init();

    let db_path = options.db_path()?;
    if let Some(dir) = db_path.parent() {
        fs::create_dir_all(dir)
            .await
            .context("failed to create db directory")?;
    }

    let db_pool = SqlitePool::connect_with(
        SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true),
    )
    .await?;

    let _block_store = BlockStore::open(db_pool).await?;

    // let repository = Repository;
    // let _mount_guard = virtual_filesystem::mount(repository, options.mount_dir)?;

    signal::ctrl_c().await?;

    Ok(())
}

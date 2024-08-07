use futures_util::StreamExt;
use ouisync::{db, protocol::RootNode};
use std::{env, ops::DerefMut, process::ExitCode};

// Tool for inspecting (and possibly to do modifications in the future) ouisync repositories.
// Right now it only prints out root nodes, but we can expand the functionality later for other
// use cases.

#[tokio::main]
async fn main() -> ExitCode {
    let arg = env::args().nth(1).unwrap_or_default();

    if arg.is_empty() {
        println!("Missing repository path");
        help();
        return ExitCode::FAILURE;
    }

    let pool = db::open_without_migrations(arg).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();

    let mut query = sqlx::query_as::<_, RootNode>(
        "SELECT
            snapshot_id,
            writer_id,
            versions,
            hash,
            signature,
            state,
            block_presence
         FROM snapshot_root_nodes
         ORDER BY snapshot_id ASC",
    )
    .fetch(conn.deref_mut());

    while let Some(value) = query.next().await {
        match value {
            Ok(value) => println!("{value:?}"),
            Err(err) => {
                println!("Error reading a row: {err:?}");
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
}

fn help() {
    println!("Usage: {} <PATH-TO-REPOSITORY>", env!("CARGO_PKG_NAME"));
    println!();
}

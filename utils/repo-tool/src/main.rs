use futures_util::{StreamExt, TryStreamExt};
use ouisync::{
    db,
    protocol::{NodeState, Proof, RootNode, Summary},
};
use sqlx::Row;
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

    let mut query = sqlx::query(
        "SELECT
            snapshot_id,
            writer_id,
            versions,
            hash,
            signature,
            block_presence
         FROM snapshot_root_nodes
         ORDER BY snapshot_id ASC",
    )
    .fetch(conn.deref_mut())
    .map_ok(|row| RootNode {
        snapshot_id: row.get(0),
        proof: Proof::new_unchecked(row.get(1), row.get(2), row.get(3), row.get(4)),
        summary: Summary {
            state: NodeState::Approved,
            block_presence: row.get(5),
        },
    });

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

//! Takes care of requesting needed blocks and removing unneeded blocks.

use super::Shared;
use crate::{
    error::{Error, Result},
    JointDirectory,
};
use std::sync::Arc;

pub(super) async fn run(shared: Arc<Shared>) {
    let mut rx = shared.index.subscribe();

    while rx.recv().await.is_ok() {
        match handle(shared.as_ref()).await {
            Ok(()) => (),
            Err(error) => {
                log::error!("block manager failed: {:?}", error);
            }
        }
    }
}

async fn handle(shared: &Shared) -> Result<()> {
    let branches = shared.branches().await?;
    let mut roots = Vec::with_capacity(branches.len());

    for branch in branches {
        match branch.open_root().await {
            Ok(dir) => roots.push(dir),
            Err(Error::BlockNotFound(_)) => {
                // TODO: request the missing block
                continue;
            }
            Err(error) => return Err(error),
        }
    }

    traverse(JointDirectory::new(None, roots)).await
}

async fn traverse(dir: JointDirectory) -> Result<()> {
    todo!()
}

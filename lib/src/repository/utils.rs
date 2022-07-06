use crate::{
    branch::Branch,
    crypto::sign::PublicKey,
    db,
    error::Result,
    joint_directory::versioned::{self, Versioned},
    version_vector::VersionVector,
};
use tokio::sync::{mpsc, oneshot};

/// Partition the branches into up-to-date and outdated
pub(super) async fn partition_branches(
    conn: &mut db::Connection,
    branches: Vec<Branch>,
    local_id: Option<&PublicKey>,
) -> Result<(Vec<Branch>, Vec<Branch>)> {
    let mut branches_with_vv = Vec::with_capacity(branches.len());

    for branch in branches {
        let vv = branch.version_vector(conn).await?;
        branches_with_vv.push((branch, vv));
    }

    let (uptodate, outdated): (Vec<_>, Vec<_>) = versioned::partition(branches_with_vv, local_id);

    Ok((
        uptodate.into_iter().map(|(branch, _)| branch).collect(),
        outdated.into_iter().map(|(branch, _)| branch).collect(),
    ))
}

impl Versioned for (Branch, VersionVector) {
    fn compare_versions(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.1.partial_cmp(&other.1)
    }

    fn branch_id(&self) -> &PublicKey {
        self.0.id()
    }
}

pub(super) async fn oneshot<T, F>(command_tx: &mpsc::Sender<T>, command_fn: F) -> Result<()>
where
    F: FnOnce(oneshot::Sender<Result<()>>) -> T,
{
    let (result_tx, result_rx) = oneshot::channel();
    command_tx.send(command_fn(result_tx)).await.unwrap_or(());

    // When this returns error it means the worker has been terminated which we treat as
    // success, for simplicity.
    result_rx.await.unwrap_or(Ok(()))
}

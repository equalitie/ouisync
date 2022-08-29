use crate::{
    crypto::sign::PublicKey,
    db,
    error::Result,
    index::BranchData,
    joint_directory::versioned::{self, Versioned},
    version_vector::VersionVector,
};
use std::sync::Arc;

/// Partition the branches into up-to-date and outdated
pub(super) async fn partition_branches(
    conn: &mut db::Connection,
    branches: Vec<Arc<BranchData>>,
    local_id: &PublicKey,
) -> Result<(Vec<Arc<BranchData>>, Vec<Arc<BranchData>>)> {
    let mut branches_with_vv = Vec::with_capacity(branches.len());

    for branch in branches {
        let vv = branch.load_version_vector(conn).await?;
        branches_with_vv.push((branch, vv));
    }

    let (uptodate, outdated): (Vec<_>, Vec<_>) =
        versioned::partition(branches_with_vv, Some(local_id));

    Ok((
        uptodate.into_iter().map(|(branch, _)| branch).collect(),
        outdated.into_iter().map(|(branch, _)| branch).collect(),
    ))
}

impl Versioned for (Arc<BranchData>, VersionVector) {
    fn compare_versions(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.1.partial_cmp(&other.1)
    }

    fn branch_id(&self) -> &PublicKey {
        self.0.id()
    }
}

use crate::{
    branch::Branch,
    crypto::sign::PublicKey,
    db,
    error::Result,
    joint_directory::versioned::{self, Versioned},
    version_vector::VersionVector,
};

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

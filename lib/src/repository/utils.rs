use crate::{
    branch::Branch,
    crypto::sign::PublicKey,
    db,
    error::Result,
    joint_directory::versioned::{self, Versioned},
    version_vector::VersionVector,
};

/// Filter outdated branches
pub(super) async fn outdated_branches(
    conn: &mut db::Connection,
    branches: Vec<Branch>,
    local_id: Option<&PublicKey>,
) -> Result<Vec<Branch>> {
    let mut branches_with_vv = Vec::new();

    for branch in branches {
        let vv = branch.version_vector(conn).await?;
        branches_with_vv.push((branch, vv));
    }

    let (_, outdated): (_, Vec<_>) = versioned::partition(branches_with_vv, local_id);

    Ok(outdated
        .into_iter()
        // Avoid deleting empty branches before any content is added to them.
        .filter(|(_, vv)| !vv.is_empty())
        .map(|(branch, _)| branch)
        .collect())
}

impl Versioned for (Branch, VersionVector) {
    fn compare_versions(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.1.partial_cmp(&other.1)
    }

    fn branch_id(&self) -> &PublicKey {
        self.0.id()
    }
}

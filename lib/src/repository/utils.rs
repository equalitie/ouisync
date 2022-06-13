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
) -> Result<Vec<Branch>> {
    let mut branches_with_vv = Vec::new();

    for branch in branches {
        let vv = branch.version_vector(conn).await?;
        branches_with_vv.push((branch, vv));
    }

    let (_, outdated): (_, Vec<_>) = versioned::partition(branches_with_vv, None);

    Ok(outdated
        .into_iter()
        // Avoid deleting empty branches before any content is added to them.
        .filter(|(_, vv)| !vv.is_empty())
        .map(|(branch, _)| branch)
        .collect())
}

impl Versioned for (Branch, VersionVector) {
    fn version_vector(&self) -> &VersionVector {
        &self.1
    }

    fn branch_id(&self) -> &PublicKey {
        self.0.id()
    }
}

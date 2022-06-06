use crate::{
    branch::Branch,
    crypto::sign::PublicKey,
    error::Result,
    joint_directory::versioned::{self, Versioned},
    version_vector::VersionVector,
};
use futures_util::{stream, StreamExt, TryStreamExt};

/// Filter outdated branches
pub(super) async fn outdated_branches(branches: Vec<Branch>) -> Result<Vec<Branch>> {
    let branches: Vec<_> = stream::iter(branches)
        .then(|branch| async move { branch.version_vector().await.map(move |vv| (branch, vv)) })
        .try_collect()
        .await?;

    let (_, outdated): (_, Vec<_>) = versioned::partition(branches, None);

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

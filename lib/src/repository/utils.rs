use crate::{branch::Branch, crypto::sign::PublicKey, error::Result, joint_directory::versioned};
use futures_util::{stream, StreamExt, TryStreamExt};
use std::collections::HashSet;

/// Filter outdated branches
pub(super) async fn outdated_branches(branches: Vec<Branch>) -> Result<Vec<Branch>> {
    let current_ids = current_branch_ids(&branches).await?;
    Ok(branches
        .into_iter()
        .filter(|branch| !current_ids.contains(branch.id()))
        .collect())
}

async fn current_branch_ids(branches: &[Branch]) -> Result<HashSet<PublicKey>> {
    let infos: Vec<_> = stream::iter(branches)
        .then(|branch| async move {
            let id = *branch.id();
            branch.version_vector().await.map(move |vv| (id, vv))
        })
        .try_collect()
        .await?;

    Ok(versioned::keep_maximal(infos, None)
        .into_iter()
        .map(|(id, _)| id)
        .collect())
}

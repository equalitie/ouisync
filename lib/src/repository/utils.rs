use crate::{branch::Branch, crypto::sign::PublicKey, joint_directory::versioned};
use futures_util::{stream, StreamExt};
use std::collections::HashSet;

/// Filter up-to-date branches
pub(super) async fn current_branches(branches: Vec<Branch>) -> Vec<Branch> {
    let current_ids = current_branch_ids(&branches).await;
    branches
        .into_iter()
        .filter(|branch| current_ids.contains(branch.id()))
        .collect()
}

/// Filter outdated branches
pub(super) async fn outdated_branches(branches: Vec<Branch>) -> Vec<Branch> {
    let current_ids = current_branch_ids(&branches).await;
    branches
        .into_iter()
        .filter(|branch| !current_ids.contains(branch.id()))
        .collect()
}

async fn current_branch_ids(branches: &[Branch]) -> HashSet<PublicKey> {
    let infos: Vec<_> = stream::iter(branches)
        .then(|branch| async move {
            let id = *branch.id();
            let vv = branch.data().root().await.proof.version_vector.clone();

            (id, vv)
        })
        .collect()
        .await;

    versioned::keep_maximal(infos, None)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

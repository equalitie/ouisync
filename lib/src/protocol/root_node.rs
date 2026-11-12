use std::cmp::Ordering;

use super::{NodeState, Proof, Summary};
use crate::{crypto::sign::PublicKey, ord};

pub(crate) type SnapshotId = u32;

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct RootNode {
    pub snapshot_id: SnapshotId,
    pub proof: Proof,
    pub summary: Summary,
}

/// Kind of root node.
#[derive(Eq, PartialEq, Debug)]
pub(crate) enum RootNodeKind {
    /// Published nodes have their version vector strictly greater than any previous node
    /// in the same branch. Only published nodes are announced to other replicas.
    Published,
    /// Draft nodes have their version vector equal to that of the previous node in the same
    /// branch. Draft nodes are neither announced to nor accepted from other replicas.
    Draft,
}

/// What kind of nodes to load and/or create.
pub(crate) enum RootNodeFilter {
    /// Only published nodes.
    Published,
    /// Published and draft nodes.
    Any,
}

/// Sort the nodes into two groups: those that are up-to-date and should be kept and those that are
/// outdated and should be pruned.
///
/// The decision into which group a node belongs to depends on its version vector, state and whether
/// the node belongs to the local branch.
///
/// # Panics
///
/// Panics if any node from the local branch is not approved. Local snapshots are always created
/// fully in a single db transaction and the quota is not applied to the local branch, so they are
/// always approved.
pub(crate) fn prune(
    nodes: Vec<RootNode>,
    local_writer_id: &PublicKey,
) -> (Vec<RootNode>, Vec<RootNode>) {
    for node in &nodes {
        assert!(
            node.proof.writer_id != *local_writer_id || node.summary.state == NodeState::Approved,
            "local root node is not approved: {node:?}"
        );
    }

    ord::partial_partition_by(nodes, |a, b| compare_for_prune(a, b, local_writer_id))
}

// Compare the nodes for the purpose of pruning according to their version vectors, whether they are
// local and whether they are approved. This is a product order of these three properties:
//
// 1. Node compares greater than the other one if its version vector happens-after or is equal to
//    the other one's, it's local or the other one isn't, and it's approved or the other one isn't,
//    and at least one of these is strict (and vice versa for less).
//
// 2. If all three properties are the same, break ties by comparing by writer id (arbitrary, but
//    consistent across replicas), then by snapshot id (assigned sequentially, so within the same
//    branch the greater one is the newer one).
//
// 3. Otherwise they are uncomparable.
//
// Additionally, two non-approved nodes from different branches are always uncomparable, regardless
// of their version vectors. This is because a non-approved node might never become approved (e.g.,
// due to the peer disconnecting before we receive all its nodes) so it can't be considered to
// supersede a node from another branch. As a consequence, a node is pruned only if there is an
// approved node greater than it, so when a whole branch is pruned, all its snapshots (including the
// older approved ones) are superseded by an approved node.
//
// Note this means an older approved node is kept when its branch has a newer, non-approved one. Both
// are pruned only when there is an approved node that is newer than both of them, either in the
// same branch or in a different one.
//
// Precondition: local nodes are always approved (checked in `prune`).
fn compare_for_prune(a: &RootNode, b: &RootNode, local_id: &PublicKey) -> Option<Ordering> {
    fn is_approved(node: &RootNode) -> bool {
        node.summary.state == NodeState::Approved
    }

    fn is_local(node: &RootNode, local_id: &PublicKey) -> bool {
        node.proof.writer_id == *local_id
    }

    if !is_approved(a) && !is_approved(b) && a.proof.writer_id != b.proof.writer_id {
        return None;
    }

    let orderings = [
        a.proof
            .version_vector
            .partial_cmp(&b.proof.version_vector)?,
        is_local(a, local_id).cmp(&is_local(b, local_id)),
        is_approved(a).cmp(&is_approved(b)),
    ];

    match (
        orderings.contains(&Ordering::Greater),
        orderings.contains(&Ordering::Less),
    ) {
        (true, true) => None,
        (true, false) => Some(Ordering::Greater),
        (false, true) => Some(Ordering::Less),
        (false, false) => Some(
            a.proof
                .writer_id
                .cmp(&b.proof.writer_id)
                .then_with(|| a.snapshot_id.cmp(&b.snapshot_id)),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::{array, sync::LazyLock};

    use proptest::collection::vec;
    use test_strategy::proptest;

    use super::*;
    use crate::{
        VersionVector,
        crypto::sign::Keypair,
        protocol::{MultiBlockPresence, NodeState},
    };

    #[test]
    fn prune_sanity_check() {
        let l_id = PublicKey::random();
        let a_id = PublicKey::random();
        let b_id = PublicKey::random();

        let a = make_root_node(a_id, VersionVector::first(a_id), NodeState::Approved);
        let b = make_root_node(b_id, VersionVector::first(b_id), NodeState::Approved);

        assert_eq!(
            prune(vec![a.clone(), b.clone()], &l_id),
            (vec![a, b], vec![])
        );
    }

    #[test]
    fn prune_different_branches_keeps_approved_when_newer_is_not_approved() {
        let l_id = PublicKey::random();
        let a_id = PublicKey::random();
        let b_id = PublicKey::random();

        let vv0 = VersionVector::first(a_id);
        let vv1 = vv0.clone().incremented(b_id);

        let a = make_root_node(a_id, vv0, NodeState::Approved);
        let b = make_root_node(b_id, vv1, NodeState::Incomplete);

        assert_eq!(
            prune(vec![a.clone(), b.clone()], &l_id),
            (vec![a.clone(), b.clone()], vec![])
        );
        assert_eq!(
            prune(vec![b.clone(), a.clone()], &l_id),
            (vec![b, a], vec![])
        );
    }

    #[test]
    fn prune_same_branch_keeps_older_approved_when_newer_is_not_approved() {
        let l_id = PublicKey::random();
        let r_id = PublicKey::random();

        let vv0 = VersionVector::first(r_id);
        let vv1 = vv0.clone().incremented(r_id);

        let a = make_root_node(r_id, vv0, NodeState::Approved);
        let b = make_root_node(r_id, vv1, NodeState::Incomplete);

        assert_eq!(
            prune(vec![a.clone(), b.clone()], &l_id),
            (vec![a.clone(), b.clone()], vec![])
        );
        assert_eq!(
            prune(vec![b.clone(), a.clone()], &l_id),
            (vec![b, a], vec![])
        );
    }

    #[test]
    fn prune_same_branch_keeps_newer_when_at_least_as_approved() {
        let l_id = PublicKey::random();
        let r_id = PublicKey::random();

        let vv0 = VersionVector::first(r_id);
        let vv1 = vv0.clone().incremented(r_id);

        for (old_state, new_state) in [
            (NodeState::Approved, NodeState::Approved),
            (NodeState::Incomplete, NodeState::Approved),
            (NodeState::Incomplete, NodeState::Incomplete),
        ] {
            let a = make_root_node(r_id, vv0.clone(), old_state);
            let b = make_root_node(r_id, vv1.clone(), new_state);

            assert_eq!(
                prune(vec![a.clone(), b.clone()], &l_id),
                (vec![b.clone()], vec![a.clone()])
            );
            assert_eq!(prune(vec![b.clone(), a.clone()], &l_id), (vec![b], vec![a]));
        }
    }

    #[test]
    fn prune_newer_approved_node_prunes_older_approved_and_newer_unapproved() {
        let l_id = PublicKey::random();
        let r_id = PublicKey::random();
        let s_id = PublicKey::random();

        let vv0 = VersionVector::first(r_id);
        let vv1 = vv0.clone().incremented(r_id);

        // The newer approved node is either local, from another remote branch or from the same
        // branch.
        for c_id in [l_id, s_id, r_id] {
            let vv2 = vv1.clone().incremented(c_id);

            let a = make_root_node(r_id, vv0.clone(), NodeState::Approved);
            let b = make_root_node(r_id, vv1.clone(), NodeState::Incomplete);
            let c = make_root_node(c_id, vv2, NodeState::Approved);

            for nodes in [
                vec![a.clone(), b.clone(), c.clone()],
                vec![c.clone(), b.clone(), a.clone()],
                vec![b.clone(), c.clone(), a.clone()],
            ] {
                let (uptodate, outdated) = prune(nodes, &l_id);

                assert_eq!(uptodate, vec![c.clone()]);
                assert_eq!(outdated.len(), 2);
                assert!(outdated.contains(&a));
                assert!(outdated.contains(&b));
            }
        }
    }

    #[test]
    fn prune_local_with_same_version_vector_keeps_newer() {
        let l_id = PublicKey::random();
        let vv = VersionVector::first(l_id);

        let a = make_root_node(l_id, vv.clone(), NodeState::Approved);
        let b = make_root_node(l_id, vv, NodeState::Approved);
        assert!(b.snapshot_id > a.snapshot_id);

        assert_eq!(
            prune(vec![a.clone(), b.clone()], &l_id),
            (vec![b.clone()], vec![a.clone()])
        );
        assert_eq!(prune(vec![b.clone(), a.clone()], &l_id), (vec![b], vec![a]));
    }

    #[test]
    #[should_panic(expected = "local root node is not approved")]
    fn prune_panics_on_unapproved_local_node() {
        let l_id = PublicKey::random();
        let a = make_root_node(l_id, VersionVector::first(l_id), NodeState::Incomplete);

        prune(vec![a], &l_id);
    }

    #[test]
    fn prune_different_branches_keeps_unapproved_when_newer_is_not_approved() {
        let l_id = PublicKey::random();
        let a_id = PublicKey::random();
        let b_id = PublicKey::random();

        let vv0 = VersionVector::first(a_id);
        let vv1 = vv0.clone().incremented(b_id);

        for a_state in [NodeState::Incomplete, NodeState::Complete] {
            for b_state in [NodeState::Incomplete, NodeState::Complete] {
                let a = make_root_node(a_id, vv0.clone(), a_state);
                let b = make_root_node(b_id, vv1.clone(), b_state);

                assert_eq!(
                    prune(vec![a.clone(), b.clone()], &l_id),
                    (vec![a.clone(), b.clone()], vec![])
                );
                assert_eq!(
                    prune(vec![b.clone(), a.clone()], &l_id),
                    (vec![b, a], vec![])
                );
            }
        }
    }

    #[test]
    fn prune_remote_with_same_writer_and_version_vector_keeps_newer() {
        let l_id = PublicKey::random();
        let r_id = PublicKey::random();
        let vv = VersionVector::first(r_id);

        for state in [NodeState::Approved, NodeState::Incomplete] {
            let a = make_root_node(r_id, vv.clone(), state);
            let b = make_root_node(r_id, vv.clone(), state);
            assert!(b.snapshot_id > a.snapshot_id);

            assert_eq!(
                prune(vec![a.clone(), b.clone()], &l_id),
                (vec![b.clone()], vec![a.clone()])
            );
            assert_eq!(prune(vec![b.clone(), a.clone()], &l_id), (vec![b], vec![a]));
        }
    }

    #[proptest]
    fn compare_for_prune_is_partial_order(
        #[strategy(vec((0..3usize, vec(0..3u64, 3), 0..2usize), 1..10))] specs: Vec<(
            usize,
            Vec<u64>,
            usize,
        )>,
    ) {
        // Writer 0 is the local one.
        let writer_ids: [PublicKey; 3] = array::from_fn(|_| PublicKey::random());
        let local_id = writer_ids[0];

        let nodes: Vec<_> = specs
            .into_iter()
            .enumerate()
            .map(|(snapshot_id, (writer_index, versions, state_index))| {
                let version_vector: VersionVector = writer_ids
                    .iter()
                    .copied()
                    .zip(versions)
                    .filter(|(_, version)| *version > 0)
                    .collect();
                // Local nodes are always approved.
                let state = if writer_index == 0 {
                    NodeState::Approved
                } else {
                    [NodeState::Approved, NodeState::Incomplete][state_index]
                };

                let mut node = make_root_node(writer_ids[writer_index], version_vector, state);
                node.snapshot_id = snapshot_id as SnapshotId;
                node
            })
            .collect();

        let cmp = |a, b| compare_for_prune(a, b, &local_id);

        for a in &nodes {
            // Reflexivity
            assert_eq!(cmp(a, a), Some(Ordering::Equal), "a = {a:?}");

            for b in &nodes {
                // Antisymmetry (also implies the duality of `Less` and `Greater`)
                assert_eq!(
                    cmp(a, b),
                    cmp(b, a).map(Ordering::reverse),
                    "a = {a:?}, b = {b:?}"
                );

                if cmp(a, b) == Some(Ordering::Equal) {
                    assert_eq!(a, b);
                }

                for c in &nodes {
                    // Transitivity
                    match (cmp(a, b), cmp(b, c)) {
                        (Some(Ordering::Less), Some(Ordering::Less | Ordering::Equal))
                        | (Some(Ordering::Equal), Some(Ordering::Less)) => {
                            assert_eq!(
                                cmp(a, c),
                                Some(Ordering::Less),
                                "a = {a:?}, b = {b:?}, c = {c:?}"
                            );
                        }
                        (Some(Ordering::Greater), Some(Ordering::Greater | Ordering::Equal))
                        | (Some(Ordering::Equal), Some(Ordering::Greater)) => {
                            assert_eq!(
                                cmp(a, c),
                                Some(Ordering::Greater),
                                "a = {a:?}, b = {b:?}, c = {c:?}"
                            );
                        }
                        (Some(Ordering::Equal), Some(Ordering::Equal)) => {
                            assert_eq!(
                                cmp(a, c),
                                Some(Ordering::Equal),
                                "a = {a:?}, b = {b:?}, c = {c:?}"
                            );
                        }
                        _ => (),
                    }
                }
            }
        }
    }

    fn make_root_node(
        writer_id: PublicKey,
        version_vector: VersionVector,
        state: NodeState,
    ) -> RootNode {
        use std::sync::atomic::{self, AtomicU32};

        static NEXT_SNAPSHOT_ID: AtomicU32 = AtomicU32::new(0);
        static WRITE_KEYS: LazyLock<Keypair> = LazyLock::new(Keypair::random);

        RootNode {
            snapshot_id: NEXT_SNAPSHOT_ID.fetch_add(1, atomic::Ordering::Relaxed),
            proof: Proof::new(writer_id, version_vector, rand::random(), &WRITE_KEYS),
            summary: Summary {
                state,
                block_presence: MultiBlockPresence::Full,
            },
        }
    }
}

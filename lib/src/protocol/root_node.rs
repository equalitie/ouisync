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
pub(crate) fn prune(
    nodes: Vec<RootNode>,
    local_writer_id: &PublicKey,
) -> (Vec<RootNode>, Vec<RootNode>) {
    ord::partial_partition_by(nodes, |a, b| compare_for_prune(a, b, local_writer_id))
}

// Compare the nodes for the purpose of pruning according to their version vectors, whether they are
// local and whether they are approved:
//
// 1. Node whose version vector is happens-after the other one compares greater, with the following
//    exceptions:
//     - If it's not local but the other one is, they are uncomparable
//     - If it's not approved but the other one is, they are uncomparable
//
// 2. If the nodes have the same version vector:
//     - If one is local and the other is not, the local one is greater.
//     - If both are local, the newer one (according to their snapshot_id) is greater, regardless
//       of whether they are approved.
//     - If both are non-local, the approved one is greater. If both are approved or both
//       non-approved, break ties by comparing by writer id (arbitrary, but consistent across
//       replicas), then by snapshot id.
//
// 3. If their version vectors are concurrent, they are uncomparable
fn compare_for_prune(a: &RootNode, b: &RootNode, local_id: &PublicKey) -> Option<Ordering> {
    fn cmp_state(a: &RootNode, b: &RootNode) -> Ordering {
        fn priority(state: NodeState) -> u8 {
            match state {
                NodeState::Approved => 1,
                NodeState::Complete | NodeState::Incomplete | NodeState::Rejected => 0,
            }
        }

        priority(a.summary.state).cmp(&priority(b.summary.state))
    }

    fn cmp_local(a: &RootNode, b: &RootNode, local_id: &PublicKey) -> Option<Ordering> {
        match (
            a.proof.writer_id == *local_id,
            b.proof.writer_id == *local_id,
        ) {
            (true, false) => Some(Ordering::Greater),
            (false, true) => Some(Ordering::Less),
            (true, true) => Some(Ordering::Equal),
            (false, false) => None,
        }
    }

    match (
        a.proof.version_vector.partial_cmp(&b.proof.version_vector),
        cmp_local(a, b, local_id),
        cmp_state(a, b),
    ) {
        (
            Some(Ordering::Greater),
            Some(Ordering::Greater) | Some(Ordering::Equal) | None,
            Ordering::Greater | Ordering::Equal,
        ) => Some(Ordering::Greater),
        (Some(Ordering::Greater), Some(Ordering::Less), _) => None,
        (Some(Ordering::Greater), _, Ordering::Less) => None,
        (Some(Ordering::Equal), Some(Ordering::Greater), _) => Some(Ordering::Greater),
        (Some(Ordering::Equal), Some(Ordering::Equal), _) => {
            Some(a.snapshot_id.cmp(&b.snapshot_id))
        }
        (Some(Ordering::Equal), Some(Ordering::Less), _) => Some(Ordering::Less),
        (Some(Ordering::Equal), None, Ordering::Greater) => Some(Ordering::Greater),
        (Some(Ordering::Equal), None, Ordering::Less) => Some(Ordering::Less),
        (Some(Ordering::Equal), None, Ordering::Equal) => Some(
            a.proof
                .writer_id
                .cmp(&b.proof.writer_id)
                .then_with(|| a.snapshot_id.cmp(&b.snapshot_id)),
        ),
        (
            Some(Ordering::Less),
            Some(Ordering::Less) | Some(Ordering::Equal) | None,
            Ordering::Less | Ordering::Equal,
        ) => Some(Ordering::Less),
        (Some(Ordering::Less), Some(Ordering::Greater), _) => None,
        (Some(Ordering::Less), _, Ordering::Greater) => None,
        (None, _, _) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

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
    fn prune_keeps_approved_when_newer_is_not_approved() {
        let l_id = PublicKey::random();

        let vv0 = VersionVector::first(l_id);
        let vv1 = vv0.clone().incremented(l_id);

        let a = make_root_node(l_id, vv0, NodeState::Approved);
        let b = make_root_node(l_id, vv1, NodeState::Incomplete);

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
    fn prune_local_with_same_version_vector_keeps_newer() {
        let l_id = PublicKey::random();
        let vv = VersionVector::first(l_id);

        // Newer one is not approved
        let a = make_root_node(l_id, vv.clone(), NodeState::Approved);
        let b = make_root_node(l_id, vv.clone(), NodeState::Incomplete);
        assert!(b.snapshot_id > a.snapshot_id);

        assert_eq!(
            prune(vec![a.clone(), b.clone()], &l_id),
            (vec![b.clone()], vec![a.clone()])
        );
        assert_eq!(prune(vec![b.clone(), a.clone()], &l_id), (vec![b], vec![a]));

        // Newer one is approved
        let a = make_root_node(l_id, vv.clone(), NodeState::Incomplete);
        let b = make_root_node(l_id, vv, NodeState::Approved);
        assert!(b.snapshot_id > a.snapshot_id);

        assert_eq!(
            prune(vec![a.clone(), b.clone()], &l_id),
            (vec![b.clone()], vec![a.clone()])
        );
        assert_eq!(prune(vec![b.clone(), a.clone()], &l_id), (vec![b], vec![a]));
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

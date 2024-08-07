use super::error::Error;
use crate::{
    crypto::{sign::PublicKey, Hash},
    db,
    debug::DebugPrinter,
    protocol::{
        BlockId, MultiBlockPresence, NodeState, Proof, RootNode, RootNodeFilter, RootNodeKind,
        SingleBlockPresence, Summary,
    },
    version_vector::VersionVector,
};
use futures_util::{Stream, StreamExt, TryStreamExt};
use sqlx::{sqlite::SqliteRow, FromRow, Row};
use std::{cmp::Ordering, fmt, future};

/// Status of receiving a root node
#[derive(PartialEq, Eq, Debug)]
pub(crate) enum RootNodeStatus {
    /// The node represents a new snapshot - write it into the store and requests its children.
    NewSnapshot,
    /// We already have the node but its block presence indicated the peer potentially has some
    /// blocks we don't have. Don't write it into the store but do request its children.
    NewBlocks,
    /// The node is outdated - discard it.
    Outdated,
}

impl RootNodeStatus {
    pub fn request_children(&self) -> bool {
        match self {
            Self::NewSnapshot | Self::NewBlocks => true,
            Self::Outdated => false,
        }
    }

    pub fn write(&self) -> bool {
        match self {
            Self::NewSnapshot => true,
            Self::NewBlocks | Self::Outdated => false,
        }
    }
}

impl fmt::Display for RootNodeStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NewSnapshot => write!(f, "new snapshot"),
            Self::NewBlocks => write!(f, "new blocks"),
            Self::Outdated => write!(f, "outdated"),
        }
    }
}

/// Creates a root node with the specified proof and summary.
///
/// A root node can be either "published" or "draft". A published node is one whose version vector
/// is strictly greater than the version vector of any previous node in the same branch.
/// A draft node is one whose version vector is equal to the version vector of the previous node.
///
/// The `filter` parameter determines what kind of node can be created.
///
/// Attempt to create a node whose version vector is less than or concurrent to the previous one is
/// not allowed.
///
/// Returns also the kind of node (published or draft) that was actually created.
pub(super) async fn create(
    tx: &mut db::WriteTransaction,
    proof: Proof,
    mut summary: Summary,
    filter: RootNodeFilter,
) -> Result<(RootNode, RootNodeKind), Error> {
    // Check that the root node to be created is newer than the latest existing root node in
    // the same branch.
    let old_vv: VersionVector = sqlx::query(
        "SELECT versions
         FROM snapshot_root_nodes
         WHERE snapshot_id = (
             SELECT MAX(snapshot_id)
             FROM snapshot_root_nodes
             WHERE writer_id = ?
         )",
    )
    .bind(&proof.writer_id)
    .map(|row| row.get(0))
    .fetch_optional(&mut *tx)
    .await?
    .unwrap_or_else(VersionVector::new);

    let kind = match (proof.version_vector.partial_cmp(&old_vv), filter) {
        (Some(Ordering::Greater), _) => RootNodeKind::Published,
        (Some(Ordering::Equal), RootNodeFilter::Any) => RootNodeKind::Draft,
        (Some(Ordering::Equal), RootNodeFilter::Published) => return Err(Error::OutdatedRootNode),
        (Some(Ordering::Less), _) => return Err(Error::OutdatedRootNode),
        (None, _) => return Err(Error::ConcurrentRootNode),
    };

    // Inherit non-incomplete state from existing nodes with the same hash.
    // (All nodes with the same hash have the same state so it's enough to fetch only the first one)
    if summary.state == NodeState::Incomplete {
        let state = sqlx::query(
            "SELECT state FROM snapshot_root_nodes WHERE hash = ? AND state <> ? LIMIT 1",
        )
        .bind(&proof.hash)
        .bind(NodeState::Incomplete)
        .fetch_optional(&mut *tx)
        .await?
        .map(|row| row.get(0));

        if let Some(state) = state {
            summary.state = state;
        }
    }

    let snapshot_id = sqlx::query(
        "INSERT INTO snapshot_root_nodes (
             writer_id,
             versions,
             hash,
             signature,
             state,
             block_presence
         )
         VALUES (?, ?, ?, ?, ?, ?)
         RETURNING snapshot_id",
    )
    .bind(&proof.writer_id)
    .bind(&proof.version_vector)
    .bind(&proof.hash)
    .bind(&proof.signature)
    .bind(summary.state)
    .bind(&summary.block_presence)
    .map(|row| row.get(0))
    .fetch_one(tx)
    .await?;

    let node = RootNode {
        snapshot_id,
        proof,
        summary,
    };

    Ok((node, kind))
}

/// Returns the latest approved root node of the specified branch.
pub(super) async fn load_latest_approved(
    conn: &mut db::Connection,
    branch_id: &PublicKey,
) -> Result<RootNode, Error> {
    sqlx::query_as(
        "SELECT
             snapshot_id,
             writer_id,
             versions,
             hash,
             signature,
             state,
             block_presence
         FROM
             snapshot_root_nodes
         WHERE
             snapshot_id = (
                 SELECT MAX(snapshot_id)
                 FROM snapshot_root_nodes
                 WHERE writer_id = ? AND state = ?
             )
        ",
    )
    .bind(branch_id)
    .bind(NodeState::Approved)
    .fetch_optional(conn)
    .await?
    .ok_or(Error::BranchNotFound)
}

/// Load the previous approved root node of the same writer.
pub(super) async fn load_prev_approved(
    conn: &mut db::Connection,
    node: &RootNode,
) -> Result<Option<RootNode>, Error> {
    sqlx::query_as(
        "SELECT
            snapshot_id,
            writer_id,
            versions,
            hash,
            signature,
            state,
            block_presence
         FROM snapshot_root_nodes
         WHERE writer_id = ? AND state = ? AND snapshot_id < ?
         ORDER BY snapshot_id DESC
         LIMIT 1",
    )
    .bind(&node.proof.writer_id)
    .bind(NodeState::Approved)
    .bind(node.snapshot_id)
    .fetch(conn)
    .err_into()
    .try_next()
    .await
}

/// Return the latest approved root nodes of all known writers.
pub(super) fn load_all_latest_approved(
    conn: &mut db::Connection,
) -> impl Stream<Item = Result<RootNode, Error>> + '_ {
    sqlx::query_as(
        "SELECT
             snapshot_id,
             writer_id,
             versions,
             hash,
             signature,
             state,
             block_presence
         FROM
             snapshot_root_nodes
         WHERE
             snapshot_id IN (
                 SELECT MAX(snapshot_id)
                 FROM snapshot_root_nodes
                 WHERE state = ?
                 GROUP BY writer_id
             )",
    )
    .bind(NodeState::Approved)
    .fetch(conn)
    .err_into()
}

/// Return the latest root nodes of all known writers according to the following order of
/// preferrence: approved, complete, incomplete, rejected. That is, returns the latest approved
/// node if it exists, otherwise the latest complete, etc...
pub(super) fn load_all_latest_preferred(
    conn: &mut db::Connection,
) -> impl Stream<Item = Result<RootNode, Error>> + '_ {
    // Partition all root nodes by their writer_id. Then sort each partition according to the
    // preferrence as described in the above doc comment. Then take the first row from each
    // partition.

    // TODO: Is this the best way to do this (simple, efficient, etc...)?

    sqlx::query_as(
        "SELECT
             snapshot_id,
             writer_id,
             versions,
             hash,
             signature,
             state,
             block_presence
         FROM (
             SELECT
                 *,
                 ROW_NUMBER() OVER (
                     PARTITION BY writer_id
                     ORDER BY
                         CASE state
                             WHEN ? THEN 0
                             WHEN ? THEN 1
                             WHEN ? THEN 2
                             WHEN ? THEN 3
                         END,
                         snapshot_id DESC
                 ) AS position
             FROM snapshot_root_nodes
         )
         WHERE position = 1",
    )
    .bind(NodeState::Approved)
    .bind(NodeState::Complete)
    .bind(NodeState::Incomplete)
    .bind(NodeState::Rejected)
    .fetch(conn)
    .err_into()
}

/// Return the latest root nodes of all known writers in any state.
pub(super) fn load_all_latest(
    conn: &mut db::Connection,
) -> impl Stream<Item = Result<RootNode, Error>> + '_ {
    sqlx::query_as(
        "SELECT
             snapshot_id,
             writer_id,
             versions,
             hash,
             signature,
             state,
             block_presence
         FROM
             snapshot_root_nodes
         WHERE
             snapshot_id IN (
                 SELECT MAX(snapshot_id)
                 FROM snapshot_root_nodes
                 GROUP BY writer_id
             )",
    )
    .fetch(conn)
    .err_into()
}

/// Returns all nodes with the specified hash
pub(super) fn load_all_by_hash<'a>(
    conn: &'a mut db::Connection,
    hash: &'a Hash,
) -> impl Stream<Item = Result<RootNode, Error>> + 'a {
    sqlx::query_as(
        "SELECT
             snapshot_id,
             writer_id,
             versions,
             hash,
             signature,
             state,
             block_presence
         FROM snapshot_root_nodes
         WHERE hash = ?",
    )
    .bind(hash)
    .fetch(conn)
    .err_into()
}

/// Loads the "best" `NodeState` of all the root nodes that reference the given missing block. If
/// the block is not referenced from any root node or if it's not missing, falls back to returning
/// `Rejected`.
pub(super) async fn load_node_state_of_missing(
    conn: &mut db::Connection,
    block_id: &BlockId,
) -> Result<NodeState, Error> {
    use NodeState as S;

    sqlx::query(
        "WITH RECURSIVE
             inner_nodes(parent) AS (
                 SELECT parent
                     FROM snapshot_leaf_nodes
                     WHERE block_id = ? AND block_presence = ?
                 UNION ALL
                 SELECT i.parent
                     FROM snapshot_inner_nodes i INNER JOIN inner_nodes c
                     WHERE i.hash = c.parent
             )
         SELECT state
             FROM snapshot_root_nodes r INNER JOIN inner_nodes c
             WHERE r.hash = c.parent",
    )
    .bind(block_id)
    .bind(SingleBlockPresence::Missing)
    .fetch(conn)
    .map_ok(|row| row.get(0))
    .err_into()
    .try_fold(S::Rejected, |old, new| {
        let new = match (old, new) {
            (S::Incomplete | S::Complete | S::Approved | S::Rejected, S::Approved)
            | (S::Approved, S::Incomplete | S::Complete | S::Rejected) => S::Approved,
            (S::Incomplete | S::Complete | S::Rejected, S::Complete)
            | (S::Complete, S::Incomplete | S::Rejected) => S::Complete,
            (S::Incomplete | S::Rejected, S::Incomplete) | (S::Incomplete, S::Rejected) => {
                S::Incomplete
            }
            (S::Rejected, S::Rejected) => S::Rejected,
        };

        future::ready(Ok(new))
    })
    .await
}

/// Does this node exist in the db?
pub(super) async fn exists(conn: &mut db::Connection, node: &RootNode) -> Result<bool, Error> {
    Ok(
        sqlx::query("SELECT 0 FROM snapshot_root_nodes WHERE snapshot_id = ?")
            .bind(node.snapshot_id)
            .fetch_optional(conn)
            .await?
            .is_some(),
    )
}

/// Removes the given root node including all its descendants that are not referenced from any
/// other root nodes.
pub(super) async fn remove(tx: &mut db::WriteTransaction, node: &RootNode) -> Result<(), Error> {
    // This uses db triggers to delete the whole snapshot.
    sqlx::query("DELETE FROM snapshot_root_nodes WHERE snapshot_id = ?")
        .bind(node.snapshot_id)
        .execute(tx)
        .await?;

    Ok(())
}

/// Removes all root nodes that are older than the given node and are on the same branch.
pub(super) async fn remove_older(
    tx: &mut db::WriteTransaction,
    node: &RootNode,
) -> Result<(), Error> {
    // This uses db triggers to delete the whole snapshot.
    sqlx::query("DELETE FROM snapshot_root_nodes WHERE snapshot_id < ? AND writer_id = ?")
        .bind(node.snapshot_id)
        .bind(&node.proof.writer_id)
        .execute(tx)
        .await?;

    Ok(())
}

/// Removes all root nodes that are older than the given node and are on the same branch and are
/// not complete.
pub(super) async fn remove_older_incomplete(
    tx: &mut db::WriteTransaction,
    node: &RootNode,
) -> Result<(), Error> {
    // This uses db triggers to delete the whole snapshot.
    sqlx::query(
        "DELETE FROM snapshot_root_nodes
         WHERE snapshot_id < ? AND writer_id = ? AND state IN (?, ?)",
    )
    .bind(node.snapshot_id)
    .bind(&node.proof.writer_id)
    .bind(NodeState::Incomplete)
    .bind(NodeState::Rejected)
    .execute(tx)
    .await?;

    Ok(())
}

/// Update the summaries of all nodes with the specified hash.
pub(super) async fn update_summaries(
    tx: &mut db::WriteTransaction,
    hash: &Hash,
    summary: Summary,
) -> Result<NodeState, Error> {
    let state = sqlx::query(
        "UPDATE snapshot_root_nodes
         SET block_presence = ?,
             state = CASE state WHEN ? THEN ? ELSE state END
         WHERE hash = ?
         RETURNING state
         ",
    )
    .bind(&summary.block_presence)
    .bind(NodeState::Incomplete)
    .bind(summary.state)
    .bind(hash)
    .fetch_optional(tx)
    .await?
    .map(|row| row.get(0))
    .unwrap_or(NodeState::Incomplete);

    Ok(state)
}

/// Check whether the `old` snapshot can serve as a fallback for the `new` snapshot.
/// A snapshot can serve as a fallback if there is at least one locator that points to a missing
/// block in `new` but present block in `old`.
pub(super) async fn check_fallback(
    conn: &mut db::Connection,
    old: &RootNode,
    new: &RootNode,
) -> Result<bool, Error> {
    // TODO: verify this query is efficient, especially on large repositories

    Ok(sqlx::query(
        "WITH RECURSIVE
             inner_nodes_old(hash) AS (
                 SELECT i.hash
                     FROM snapshot_inner_nodes AS i
                     INNER JOIN snapshot_root_nodes AS r ON r.hash = i.parent
                     WHERE r.snapshot_id = ?
                 UNION ALL
                 SELECT c.hash
                     FROM snapshot_inner_nodes AS c
                     INNER JOIN inner_nodes_old AS p ON p.hash = c.parent
             ),
             inner_nodes_new(hash) AS (
                 SELECT i.hash
                     FROM snapshot_inner_nodes AS i
                     INNER JOIN snapshot_root_nodes AS r ON r.hash = i.parent
                     WHERE r.snapshot_id = ?
                 UNION ALL
                 SELECT c.hash
                     FROM snapshot_inner_nodes AS c
                     INNER JOIN inner_nodes_new AS p ON p.hash = c.parent
             )
         SELECT locator
             FROM snapshot_leaf_nodes
             WHERE block_presence = ? AND parent IN inner_nodes_old
         INTERSECT
         SELECT locator
             FROM snapshot_leaf_nodes
             WHERE block_presence = ? AND parent IN inner_nodes_new
         LIMIT 1",
    )
    .bind(old.snapshot_id)
    .bind(new.snapshot_id)
    .bind(SingleBlockPresence::Present)
    .bind(SingleBlockPresence::Missing)
    .fetch_optional(conn)
    .await?
    .is_some())
}

/// Approve the nodes with the specified hash.
pub(super) async fn approve(tx: &mut db::WriteTransaction, hash: &Hash) -> Result<(), Error> {
    set_state(tx, hash, NodeState::Approved).await
}

/// Reject the nodes with the specified hash.
pub(super) async fn reject(tx: &mut db::WriteTransaction, hash: &Hash) -> Result<(), Error> {
    set_state(tx, hash, NodeState::Rejected).await
}

async fn set_state(
    tx: &mut db::WriteTransaction,
    hash: &Hash,
    state: NodeState,
) -> Result<(), Error> {
    sqlx::query("UPDATE snapshot_root_nodes SET state = ? WHERE hash = ?")
        .bind(state)
        .bind(hash)
        .execute(tx)
        .await?;
    Ok(())
}

/// Returns all writer ids.
pub(super) fn load_writer_ids(
    conn: &mut db::Connection,
) -> impl Stream<Item = Result<PublicKey, Error>> + '_ {
    sqlx::query("SELECT DISTINCT writer_id FROM snapshot_root_nodes")
        .fetch(conn)
        .map_ok(|row| row.get(0))
        .err_into()
}

/// Returns the writer ids of the nodes with the specified hash.
pub(super) fn load_writer_ids_by_hash<'a>(
    conn: &'a mut db::Connection,
    hash: &'a Hash,
) -> impl Stream<Item = Result<PublicKey, Error>> + 'a {
    sqlx::query("SELECT DISTINCT writer_id FROM snapshot_root_nodes WHERE hash = ?")
        .bind(hash)
        .fetch(conn)
        .map_ok(|row| row.get(0))
        .err_into()
}

pub(super) async fn status(
    conn: &mut db::Connection,
    new_proof: &Proof,
    new_block_presence: &MultiBlockPresence,
) -> Result<RootNodeStatus, Error> {
    let mut status = RootNodeStatus::NewSnapshot;

    let mut old_nodes = load_all_latest(conn);
    while let Some(old_node) = old_nodes.try_next().await? {
        match new_proof
            .version_vector
            .partial_cmp(&old_node.proof.version_vector)
        {
            Some(Ordering::Less) => {
                // The incoming node is outdated compared to at least one existing node - discard
                // it.
                status = RootNodeStatus::Outdated;
            }
            Some(Ordering::Equal) => {
                if new_proof.hash == old_node.proof.hash {
                    // The incoming node has the same version vector and the same hash as one of
                    // the existing nodes which means its effectively the same node (except
                    // possibly in a different branch). There is no point inserting it but if the
                    // incoming summary is potentially more up-to-date than the exising one, we
                    // still want to request the children. Otherwise we discard it.
                    if old_node
                        .summary
                        .block_presence
                        .is_outdated(new_block_presence)
                    {
                        status = RootNodeStatus::NewBlocks;
                    } else {
                        status = RootNodeStatus::Outdated;
                    }
                } else {
                    tracing::warn!(
                        vv = ?old_node.proof.version_vector,
                        old_writer_id = ?old_node.proof.writer_id,
                        new_writer_id = ?new_proof.writer_id,
                        old_hash = ?old_node.proof.hash,
                        new_hash = ?new_proof.hash,
                        "Received root node invalid - broken invariant: same vv but different hash"
                    );

                    status = RootNodeStatus::Outdated;
                }
            }
            Some(Ordering::Greater) => (),
            None => {
                if new_proof.writer_id == old_node.proof.writer_id {
                    tracing::warn!(
                        old_vv = ?old_node.proof.version_vector,
                        new_vv = ?new_proof.version_vector,
                        writer_id = ?new_proof.writer_id,
                        "Received root node invalid - broken invariant: concurrency within branch is not allowed"
                    );

                    status = RootNodeStatus::Outdated;
                }
            }
        }

        if matches!(status, RootNodeStatus::Outdated) {
            break;
        }
    }

    Ok(status)
}

pub(super) async fn debug_print(conn: &mut db::Connection, printer: DebugPrinter) {
    let mut roots = sqlx::query_as::<_, RootNode>(
        "SELECT
             snapshot_id,
             writer_id,
             versions,
             hash,
             signature,
             state,
             block_presence
         FROM snapshot_root_nodes
         ORDER BY snapshot_id DESC",
    )
    .fetch(conn);

    while let Some(root_node) = roots.next().await {
        match root_node {
            Ok(root_node) => {
                printer.debug(&format_args!(
                    "RootNode: snapshot_id:{:?}, writer_id:{:?}, vv:{:?}, state:{:?}",
                    root_node.snapshot_id,
                    root_node.proof.writer_id,
                    root_node.proof.version_vector,
                    root_node.summary.state
                ));
            }
            Err(err) => {
                printer.debug(&format_args!("RootNode: error: {:?}", err));
            }
        }
    }
}

/// Returns a stream of all root nodes corresponding to the specified writer ordered from the
/// most recent to the least recent.
#[cfg(test)]
pub(super) fn load_all_by_writer<'a>(
    conn: &'a mut db::Connection,
    writer_id: &'a PublicKey,
) -> impl Stream<Item = Result<RootNode, Error>> + 'a {
    sqlx::query_as(
        "SELECT
             snapshot_id,
             writer_id,
             versions,
             hash,
             signature,
             state,
             block_presence
         FROM snapshot_root_nodes
         WHERE writer_id = ?
         ORDER BY snapshot_id DESC",
    )
    .bind(writer_id) // needed to satisfy the borrow checker.
    .fetch(conn)
    .err_into()
}

impl FromRow<'_, SqliteRow> for RootNode {
    fn from_row(row: &SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(RootNode {
            snapshot_id: row.try_get(0)?,
            proof: Proof::new_unchecked(
                row.try_get(1)?,
                row.try_get(2)?,
                row.try_get(3)?,
                row.try_get(4)?,
            ),
            summary: Summary {
                state: row.try_get(5)?,
                block_presence: row.try_get(6)?,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::sign::Keypair;
    use assert_matches::assert_matches;
    use tempfile::TempDir;

    #[tokio::test]
    async fn create_new() {
        let (_base_dir, pool) = setup().await;

        let writer_id = PublicKey::random();
        let write_keys = Keypair::random();
        let hash = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let (node0, _) = create(
            &mut tx,
            Proof::new(
                writer_id,
                VersionVector::first(writer_id),
                hash,
                &write_keys,
            ),
            Summary::INCOMPLETE,
            RootNodeFilter::Any,
        )
        .await
        .unwrap();
        assert_eq!(node0.proof.hash, hash);

        let nodes: Vec<_> = load_all_by_writer(&mut tx, &writer_id)
            .try_collect()
            .await
            .unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0], node0);
    }

    #[tokio::test]
    async fn create_draft() {
        let (_base_dir, pool) = setup().await;

        let writer_id = PublicKey::random();
        let write_keys = Keypair::random();

        let mut tx = pool.begin_write().await.unwrap();

        let (node0, kind) = create(
            &mut tx,
            Proof::new(
                writer_id,
                VersionVector::first(writer_id),
                rand::random(),
                &write_keys,
            ),
            Summary::INCOMPLETE,
            RootNodeFilter::Any,
        )
        .await
        .unwrap();
        assert_eq!(kind, RootNodeKind::Published);

        let (_node1, kind) = create(
            &mut tx,
            Proof::new(
                writer_id,
                node0.proof.version_vector.clone(),
                rand::random(),
                &write_keys,
            ),
            Summary::INCOMPLETE,
            RootNodeFilter::Any,
        )
        .await
        .unwrap();
        assert_eq!(kind, RootNodeKind::Draft);
    }

    #[tokio::test]
    async fn attempt_to_create_outdated_node() {
        let (_base_dir, pool) = setup().await;

        let writer_id = PublicKey::random();
        let write_keys = Keypair::random();
        let hash = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let vv0 = VersionVector::first(writer_id);
        let vv1 = vv0.clone().incremented(writer_id);

        create(
            &mut tx,
            Proof::new(writer_id, vv1.clone(), hash, &write_keys),
            Summary::INCOMPLETE,
            RootNodeFilter::Any,
        )
        .await
        .unwrap();

        // Same vv
        assert_matches!(
            create(
                &mut tx,
                Proof::new(writer_id, vv1, hash, &write_keys),
                Summary::INCOMPLETE,
                RootNodeFilter::Published, // With `RootNodeFilter::Any` this would be allowed
            )
            .await,
            Err(Error::OutdatedRootNode)
        );

        // Old vv
        assert_matches!(
            create(
                &mut tx,
                Proof::new(writer_id, vv0, hash, &write_keys),
                Summary::INCOMPLETE,
                RootNodeFilter::Any,
            )
            .await,
            Err(Error::OutdatedRootNode)
        );
    }

    // Proptest for the `load_all_latest_preferred` function.
    mod load_all_latest_preferred {
        use super::*;
        use crate::protocol::SnapshotId;
        use proptest::{arbitrary::any, collection::vec, sample::select, strategy::Strategy};
        use test_strategy::proptest;

        #[proptest]
        fn proptest(
            write_keys: Keypair,
            #[strategy(root_node_params_strategy())] input: Vec<(
                SnapshotId,
                PublicKey,
                Hash,
                NodeState,
            )>,
        ) {
            crate::test_utils::run(case(write_keys, input))
        }

        async fn case(write_keys: Keypair, input: Vec<(SnapshotId, PublicKey, Hash, NodeState)>) {
            let (_base_dir, pool) = setup().await;

            let mut writer_ids: Vec<_> = input
                .iter()
                .map(|(_, writer_id, _, _)| *writer_id)
                .collect();
            writer_ids.sort();
            writer_ids.dedup();

            let mut expected: Vec<_> = writer_ids
                .into_iter()
                .filter_map(|this_writer_id| {
                    input
                        .iter()
                        .filter(|(_, that_writer_id, _, _)| *that_writer_id == this_writer_id)
                        .map(|(snapshot_id, _, _, state)| (*snapshot_id, *state))
                        .max_by_key(|(snapshot_id, state)| {
                            (
                                match state {
                                    NodeState::Approved => 3,
                                    NodeState::Complete => 2,
                                    NodeState::Incomplete => 1,
                                    NodeState::Rejected => 0,
                                },
                                *snapshot_id,
                            )
                        })
                        .map(|(snapshot_id, state)| (this_writer_id, snapshot_id, state))
                })
                .collect();
            expected.sort_by_key(|(writer_id, _, _)| *writer_id);

            let mut vv = VersionVector::default();
            let mut tx = pool.begin_write().await.unwrap();

            for (expected_snapshot_id, writer_id, hash, state) in input {
                vv.increment(writer_id);

                let (node, _) = create(
                    &mut tx,
                    Proof::new(writer_id, vv.clone(), hash, &write_keys),
                    Summary {
                        state,
                        block_presence: MultiBlockPresence::None,
                    },
                    RootNodeFilter::Any,
                )
                .await
                .unwrap();

                assert_eq!(node.snapshot_id, expected_snapshot_id);
            }

            let mut actual: Vec<_> = load_all_latest_preferred(&mut tx)
                .map_ok(|node| (node.proof.writer_id, node.snapshot_id, node.summary.state))
                .try_collect()
                .await
                .unwrap();
            actual.sort_by_key(|(writer_id, _, _)| *writer_id);

            assert_eq!(actual, expected);

            drop(tx);
            pool.close().await.unwrap();
        }

        fn root_node_params_strategy(
        ) -> impl Strategy<Value = Vec<(SnapshotId, PublicKey, Hash, NodeState)>> {
            vec(any::<PublicKey>(), 1..=3)
                .prop_flat_map(|writer_ids| {
                    vec(
                        (select(writer_ids), any::<Hash>(), any::<NodeState>()),
                        0..=32,
                    )
                })
                .prop_map(|params| {
                    params
                        .into_iter()
                        .enumerate()
                        .map(|(index, (writer_id, hash, state))| {
                            ((index + 1) as u32, writer_id, hash, state)
                        })
                        .collect()
                })
        }
    }

    async fn setup() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }
}

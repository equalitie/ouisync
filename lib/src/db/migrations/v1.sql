--------------------------------------------------------------------------------
--
-- Repository metadata
--
--------------------------------------------------------------------------------

-- For storing unencrypted values
CREATE TABLE IF NOT EXISTS metadata_public (
    name  BLOB NOT NULL PRIMARY KEY,
    value BLOB NOT NULL
) WITHOUT ROWID;

-- For storing encrypted values
CREATE TABLE IF NOT EXISTS metadata_secret (
    name     BLOB NOT NULL PRIMARY KEY,
    nonce    BLOB NOT NULL,
    value    BLOB NOT NULL,

    UNIQUE(nonce)
) WITHOUT ROWID;



--------------------------------------------------------------------------------
--
-- Index
--
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS snapshot_root_nodes (
    snapshot_id             INTEGER PRIMARY KEY,
    writer_id               BLOB NOT NULL,
    versions                BLOB NOT NULL,

    -- Hash of the children
    hash                    BLOB NOT NULL,

    -- Signature proving the creator has write access
    signature               BLOB NOT NULL,

    -- Is this snapshot completely downloaded?
    is_complete             INTEGER NOT NULL,

    -- Summary of the missing blocks in this subree
    missing_blocks_count    INTEGER NOT NULL,
    missing_blocks_checksum INTEGER NOT NULL,

    UNIQUE(writer_id, hash)
);

CREATE INDEX IF NOT EXISTS index_snapshot_root_nodes_on_hash
    ON snapshot_root_nodes (hash);

CREATE TABLE IF NOT EXISTS snapshot_inner_nodes (
    -- Parent's `hash`
    parent                  BLOB NOT NULL,

    -- Index of this node within its siblings
    bucket                  INTEGER NOT NULL,

    -- Hash of the children
    hash                    BLOB NOT NULL,

    -- Is this subree completely downloaded?
    is_complete             INTEGER NOT NULL,

    -- Summary of the missing blocks in this subree
    missing_blocks_count    INTEGER NOT NULL,
    missing_blocks_checksum INTEGER NOT NULL,

    UNIQUE(parent, bucket)
);

CREATE INDEX IF NOT EXISTS index_snapshot_inner_nodes_on_hash
    ON snapshot_inner_nodes (hash);

CREATE TABLE IF NOT EXISTS snapshot_leaf_nodes (
    -- Parent's `hash`
    parent      BLOB NOT NULL,
    locator     BLOB NOT NULL,
    block_id    BLOB NOT NULL,

    -- Is the block pointed to by this node missing?
    is_missing  INTEGER NOT NULL,

    UNIQUE(parent, locator, block_id)
);

CREATE INDEX IF NOT EXISTS index_snapshot_leaf_nodes_on_block_id
    ON snapshot_leaf_nodes (block_id);

-- Prevents creating multiple inner nodes with the same parent and bucket but different
-- hash.
CREATE TRIGGER IF NOT EXISTS snapshot_inner_nodes_conflict_check
BEFORE INSERT ON snapshot_inner_nodes
WHEN EXISTS (
    SELECT 0
    FROM snapshot_inner_nodes
    WHERE parent = new.parent
      AND bucket = new.bucket
      AND hash <> new.hash
)
BEGIN
    SELECT RAISE (ABORT, 'inner node conflict');
END;

-- Delete whole subtree if a node is deleted and there are no more nodes at the same layer
-- with the same hash.
-- Note this needs `PRAGMA recursive_triggers = ON` to work.
CREATE TRIGGER IF NOT EXISTS snapshot_inner_nodes_delete_on_root_deleted
AFTER DELETE ON snapshot_root_nodes
WHEN NOT EXISTS (SELECT 0 FROM snapshot_root_nodes WHERE hash = old.hash)
BEGIN
    DELETE FROM snapshot_inner_nodes WHERE parent = old.hash;
END;

CREATE TRIGGER IF NOT EXISTS snapshot_inner_nodes_delete_on_parent_deleted
AFTER DELETE ON snapshot_inner_nodes
WHEN NOT EXISTS (SELECT 0 FROM snapshot_inner_nodes WHERE hash = old.hash)
BEGIN
    DELETE FROM snapshot_inner_nodes WHERE parent = old.hash;
END;

CREATE TRIGGER IF NOT EXISTS snapshot_leaf_nodes_delete_on_parent_deleted
AFTER DELETE ON snapshot_inner_nodes
WHEN NOT EXISTS (SELECT 0 FROM snapshot_inner_nodes WHERE hash = old.hash)
BEGIN
    DELETE FROM snapshot_leaf_nodes WHERE parent = old.hash;
END;



--------------------------------------------------------------------------------
--
-- Block store
--
--------------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS blocks (
    id       BLOB NOT NULL PRIMARY KEY,
    nonce    BLOB NOT NULL,
    content  BLOB NOT NULL
) WITHOUT ROWID;

CREATE TEMPORARY TABLE IF NOT EXISTS reachable_blocks (
    id     BLOB    NOT NULL PRIMARY KEY,
    pinned INTEGER NOT NULL
) WITHOUT ROWID;

-- Delete orphaned blocks.
CREATE TRIGGER IF NOT EXISTS blocks_delete_on_leaf_node_deleted
AFTER DELETE ON snapshot_leaf_nodes
WHEN NOT EXISTS (SELECT 0 FROM snapshot_leaf_nodes WHERE block_id = old.block_id)
BEGIN
    DELETE FROM blocks WHERE id = old.block_id;
END;



--------------------------------------------------------------------------------
--
-- Receive filter
--
--------------------------------------------------------------------------------
CREATE TEMPORARY TABLE IF NOT EXISTS received_inner_nodes (
    client_id               INTEGER NOT NULL,
    hash                    BLOB NOT NULL,
    missing_blocks_count    INTEGER NOT NULL,
    missing_blocks_checksum INTEGER NOT NULL,
    UNIQUE(client_id, hash)
);

CREATE INDEX IF NOT EXISTS index_received_inner_nodes_on_hash
    ON received_inner_nodes (hash);

-- Delete from received_inner_nodes if the corresponding snapshot_inner_nodes row is
-- deleted
CREATE TEMPORARY TRIGGER IF NOT EXISTS received_inner_nodes_delete_on_snapshot_deleted
AFTER DELETE ON snapshot_inner_nodes
WHEN NOT EXISTS (SELECT 0 FROM snapshot_inner_nodes WHERE hash = old.hash)
BEGIN
    DELETE FROM received_inner_nodes WHERE hash = old.hash;
END;

-- Delete from received_inner_nodes if the corresponding snapshot_inner_nodes row has no
-- missing blocks
CREATE TEMPORARY TRIGGER IF NOT EXISTS
    received_inner_nodes_delete_on_no_blocks_missing_after_insert
AFTER INSERT ON snapshot_inner_nodes
WHEN new.missing_blocks_count = 0
BEGIN
    DELETE FROM received_inner_nodes WHERE hash = new.hash;
END;

CREATE TEMPORARY TRIGGER IF NOT EXISTS
    received_inner_nodes_delete_on_no_blocks_missing_after_update
AFTER UPDATE ON snapshot_inner_nodes
WHEN new.missing_blocks_count = 0
BEGIN
    DELETE FROM received_inner_nodes WHERE hash = new.hash;
END;

--------------------------------------------------------------------------------
--
-- Temporary tables
--
-- This is always executed, regardless of what the storage version is, because
-- these table are automatically dropped when the app terminates. Because of
-- this any changes to the temp tables can be done by editing this file
-- directly and don't require creating a separate migration file.
--
--------------------------------------------------------------------------------


-- Unreachable blocks
CREATE TEMPORARY TABLE IF NOT EXISTS unreachable_blocks (
    id BLOB NOT NULL PRIMARY KEY
) WITHOUT ROWID;

-- Receive filter
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

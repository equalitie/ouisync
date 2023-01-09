--------------------------------------------------------------------------------
--
-- Remove uniqueness constraint from (writer_id, hash) in snapshot_root_nodes to allow creating
-- root nodes with the same writer_id + hash but different (greater) version vector (the version
-- vector check is done outside of the database)
--
--------------------------------------------------------------------------------

-- Due to limitations of the ALTER TABLE command in sqlite we need to create a new table with the
-- same schema except the uniqueness constraint, then copy all data from the old table to the new
-- table, then drop the old table

-- Rename the existing table
ALTER TABLE snapshot_root_nodes RENAME TO snapshot_root_nodes_old;

-- Create the new table
CREATE TABLE snapshot_root_nodes (
    snapshot_id    INTEGER PRIMARY KEY,
    writer_id      BLOB NOT NULL,
    versions       BLOB NOT NULL,
    hash           BLOB NOT NULL,
    signature      BLOB NOT NULL,
    is_complete    INTEGER NOT NULL,
    block_presence BLOB NOT NULL
);

-- Recreate indices
CREATE INDEX IF NOT EXISTS index_snapshot_root_nodes_on_hash
    ON snapshot_root_nodes (hash);

-- This is a unique index in the old table but a non-unique one in the new
CREATE INDEX IF NOT EXISTS index_snapshot_root_nodes_on_writer_id_and_hash
    ON snapshot_root_nodes (writer_id, hash);

-- Transfer rows
INSERT INTO snapshot_root_nodes SELECT * FROM snapshot_root_nodes_old;

-- Drop the old table
DROP TABLE snapshot_root_nodes_old;

-- Recreate triggers
CREATE TRIGGER snapshot_inner_nodes_delete_on_root_deleted
AFTER DELETE ON snapshot_root_nodes
WHEN NOT EXISTS (SELECT 0 FROM snapshot_root_nodes WHERE hash = old.hash)
BEGIN
    DELETE FROM snapshot_inner_nodes WHERE parent = old.hash;
END

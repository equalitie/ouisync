ALTER TABLE snapshot_root_nodes RENAME COLUMN missing_blocks_count TO block_presence;
ALTER TABLE snapshot_root_nodes DROP COLUMN missing_blocks_checksum;

-- If there were zero missing blocks, set the block_presence to 0xffffffffffffffff to indicate that
-- no blocks are missing. Otherwise set it to 0 indicating that all blocks are missing to force the
-- value to be updated on the next sync.
UPDATE snapshot_root_nodes SET
    block_presence = CASE block_presence
        WHEN 0 THEN 0xffffffffffffffff
        ELSE 0
    END;

ALTER TABLE snapshot_inner_nodes RENAME COLUMN missing_blocks_count TO block_presence;
ALTER TABLE snapshot_inner_nodes DROP COLUMN missing_blocks_checksum;

-- Same as for snapshot_root_nodes
UPDATE snapshot_inner_nodes SET
    block_presence = CASE block_presence
        WHEN 0 THEN 0xffffffffffffffff
        ELSE 0
END;

ALTER TABLE received_inner_nodes RENAME COLUMN missing_blocks_count TO block_presence;
ALTER TABLE received_inner_nodes DROP COLUMN missing_blocks_checksum;
DELETE FROM received_inner_nodes;

DROP   TRIGGER IF EXISTS received_inner_nodes_delete_on_no_blocks_missing_after_insert;
CREATE TRIGGER           received_inner_nodes_delete_on_no_blocks_missing_after_insert
AFTER INSERT ON snapshot_inner_nodes
WHEN new.block_presence = 0xffffffffffffffff -- FULL
BEGIN
    DELETE FROM received_inner_nodes WHERE hash = new.hash;
END;

DROP   TRIGGER IF EXISTS received_inner_nodes_delete_on_no_blocks_missing_after_update;
CREATE TRIGGER           received_inner_nodes_delete_on_no_blocks_missing_after_update
AFTER UPDATE ON snapshot_inner_nodes
WHEN new.block_presence = 0xffffffffffffffff -- FULL
BEGIN
    DELETE FROM received_inner_nodes WHERE hash = new.hash;
END;


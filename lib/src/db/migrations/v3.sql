ALTER TABLE snapshot_root_nodes RENAME COLUMN missing_blocks_count    TO block_presence;
ALTER TABLE snapshot_root_nodes RENAME COLUMN missing_blocks_checksum TO block_presence_checksum;
UPDATE snapshot_root_nodes SET block_presence = 0, block_presence_checksum = 0;

ALTER TABLE snapshot_inner_nodes RENAME COLUMN missing_blocks_count    TO block_presence;
ALTER TABLE snapshot_inner_nodes RENAME COLUMN missing_blocks_checksum TO block_presence_checksum;
UPDATE snapshot_inner_nodes SET block_presence = 0, block_presence_checksum = 0;

ALTER TABLE received_inner_nodes RENAME COLUMN missing_blocks_count    TO block_presence;
ALTER TABLE received_inner_nodes RENAME COLUMN missing_blocks_checksum TO block_presence_checksum;
DELETE FROM received_inner_nodes;

DROP   TRIGGER IF EXISTS     received_inner_nodes_delete_on_no_blocks_missing_after_insert;
CREATE TRIGGER IF NOT EXISTS received_inner_nodes_delete_on_no_blocks_missing_after_insert
AFTER INSERT ON snapshot_inner_nodes
WHEN new.block_presence = 2 -- 2 == Full
BEGIN
    DELETE FROM received_inner_nodes WHERE hash = new.hash;
END;

DROP   TRIGGER IF EXISTS     received_inner_nodes_delete_on_no_blocks_missing_after_update;
CREATE TRIGGER IF NOT EXISTS received_inner_nodes_delete_on_no_blocks_missing_after_update
AFTER UPDATE ON snapshot_inner_nodes
WHEN new.block_presence = 2 -- 2 == Full
BEGIN
    DELETE FROM received_inner_nodes WHERE hash = new.hash;
END;


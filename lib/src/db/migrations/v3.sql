--
-- received_inner_nodes 1/2
--
DROP TRIGGER IF EXISTS received_inner_nodes_delete_on_no_blocks_missing_after_insert;
DROP TRIGGER IF EXISTS received_inner_nodes_delete_on_no_blocks_missing_after_update;

--
-- snapshot_root_nodes
--
ALTER TABLE snapshot_root_nodes ADD COLUMN block_presence BLOB NOT NULL DEFAULT x'';

-- If there were zero missing blocks, set the block_presence to `u128::MAX` to indicate that no
-- blocks are missing. Otherwise set it to 0 indicating that all blocks are missing to force the
-- value to be updated on the next sync.
UPDATE snapshot_root_nodes SET
    block_presence = CASE missing_blocks_count
        WHEN 0 THEN x'ffffffffffffffffffffffffffffffff'
        ELSE        x'00000000000000000000000000000000'
    END;

ALTER TABLE snapshot_root_nodes DROP COLUMN missing_blocks_count;
ALTER TABLE snapshot_root_nodes DROP COLUMN missing_blocks_checksum;

--
-- snapshot_inner_nodes
--
ALTER TABLE snapshot_inner_nodes ADD COLUMN block_presence BLOB NOT NULL DEFAULT x'';

UPDATE snapshot_inner_nodes SET
    block_presence = CASE missing_blocks_count
        WHEN 0 THEN x'ffffffffffffffffffffffffffffffff'
        ELSE        x'00000000000000000000000000000000'
END;

ALTER TABLE snapshot_inner_nodes DROP COLUMN missing_blocks_count;
ALTER TABLE snapshot_inner_nodes DROP COLUMN missing_blocks_checksum;

--
-- received_inner_nodes 2/2
--
ALTER TABLE received_inner_nodes ADD COLUMN block_presence BLOB NOT NULL DEFAULT x'';
ALTER TABLE received_inner_nodes DROP COLUMN missing_blocks_count;
ALTER TABLE received_inner_nodes DROP COLUMN missing_blocks_checksum;
DELETE FROM received_inner_nodes;

CREATE TRIGGER received_inner_nodes_delete_on_no_blocks_missing_after_insert
AFTER INSERT ON snapshot_inner_nodes
WHEN new.block_presence = x'ffffffffffffffffffffffffffffffff' -- Full
BEGIN
    DELETE FROM received_inner_nodes WHERE hash = new.hash;
END;

CREATE TRIGGER received_inner_nodes_delete_on_no_blocks_missing_after_update
AFTER UPDATE ON snapshot_inner_nodes
WHEN new.block_presence = x'ffffffffffffffffffffffffffffffff' -- Full
BEGIN
    DELETE FROM received_inner_nodes WHERE hash = new.hash;
END;


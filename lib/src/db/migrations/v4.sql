-- Rename `is_missing` to `block_presence` and invert the values (1 - present, 0 - missing)
ALTER TABLE snapshot_leaf_nodes RENAME COLUMN is_missing TO block_presence;
UPDATE snapshot_leaf_nodes SET block_presence = CASE block_presence WHEN 0 THEN 1 ELSE 0 END;

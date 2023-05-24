--------------------------------------------------------------------------------
--
-- Change is_complete -> state in snapshot_root_nodes to support more that two
-- states:
--
--     0 - incomplete
--     1 - pending
--     2 - approved
--     3 - rejected
--
-- This is to support storage quota enforcement.
--
--------------------------------------------------------------------------------

ALTER TABLE snapshot_root_nodes  RENAME COLUMN is_complete TO state;
ALTER TABLE snapshot_inner_nodes RENAME COLUMN is_complete TO state;

-- Initially change all complete to approved
UPDATE snapshot_root_nodes SET state = 2 WHERE state = 1;


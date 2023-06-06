-- Fix bug in receive filter where entries were prematurely removed on snapshot becoming complete,
-- negating the purpose of the filter.

-- We want to cache root nodes as well
ALTER TABLE received_inner_nodes RENAME TO received_nodes;

-- Remove the buggy trigger
DROP TRIGGER received_inner_nodes_delete_on_node_update;


-- Delete from received_inner_nodes when the corresponding entry in snapshot_inner_nodes is
-- updated
DROP TRIGGER received_inner_nodes_delete_on_no_blocks_missing_after_update;

CREATE TRIGGER received_inner_nodes_delete_on_node_update
AFTER UPDATE ON snapshot_inner_nodes
BEGIN
    DELETE FROM received_inner_nodes WHERE hash = new.hash;
END;


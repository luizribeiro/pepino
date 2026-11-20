CREATE INDEX task_register_edges ON task_register (board, hlc, id)
WHERE kind = 'edge' AND value IS NOT NULL;

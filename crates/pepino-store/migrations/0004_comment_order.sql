CREATE INDEX task_register_comments ON task_register (board, task, hlc, id)
WHERE kind = 'comment';

CREATE TABLE task_register (
    board BLOB NOT NULL,
    task BLOB NOT NULL,
    kind TEXT NOT NULL,
    member BLOB NOT NULL,
    hlc BLOB NOT NULL CHECK (length(hlc) = 12),
    id BLOB NOT NULL,
    value BLOB,
    PRIMARY KEY (board, task, kind, member),
    FOREIGN KEY (board, task) REFERENCES task_state (board, task) ON DELETE CASCADE,
    FOREIGN KEY (board, id) REFERENCES event_log (board, id)
) STRICT, WITHOUT ROWID;

CREATE TABLE event_task (
    board BLOB NOT NULL,
    task BLOB NOT NULL CHECK (length(task) = 32),
    hlc BLOB NOT NULL CHECK (length(hlc) = 12),
    id BLOB NOT NULL,
    PRIMARY KEY (board, task, hlc, id),
    FOREIGN KEY (board, id) REFERENCES event_log (board, id)
) STRICT, WITHOUT ROWID;

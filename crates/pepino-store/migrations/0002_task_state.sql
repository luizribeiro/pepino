CREATE TABLE task_state (
    board BLOB NOT NULL CHECK (length(board) = 32),
    task BLOB NOT NULL CHECK (length(task) = 32),
    created BLOB CHECK (length(created) = 12),
    author BLOB CHECK (length(author) = 32),
    updated BLOB NOT NULL CHECK (length(updated) = 12),
    deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
    CHECK ((created IS NULL) = (author IS NULL)),
    PRIMARY KEY (board, task)
) STRICT, WITHOUT ROWID;

CREATE VIEW visible_tasks AS
    SELECT board, task, created, author, updated FROM task_state
    WHERE created IS NOT NULL AND deleted = 0;

CREATE TABLE publication (
    board BLOB NOT NULL,
    event BLOB NOT NULL,
    operation BLOB NOT NULL CHECK (length(operation) = 32),
    PRIMARY KEY (board, event),
    FOREIGN KEY (board, event) REFERENCES event_log (board, id)
) STRICT, WITHOUT ROWID;

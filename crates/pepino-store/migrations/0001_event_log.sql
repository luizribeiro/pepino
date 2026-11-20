CREATE TABLE event_log (
    board BLOB NOT NULL CHECK (length(board) = 32),
    id BLOB NOT NULL CHECK (length(id) = 32),
    hlc BLOB CHECK (length(hlc) = 12),
    author BLOB CHECK (length(author) = 32),
    classification TEXT NOT NULL CHECK (classification IN (
        'known', 'unknown_version', 'unknown_event', 'unknown_value', 'invalid'
    )),
    bytes BLOB NOT NULL,
    PRIMARY KEY (board, id)
) STRICT, WITHOUT ROWID;

CREATE INDEX event_log_clock ON event_log (board, hlc DESC) WHERE hlc IS NOT NULL;

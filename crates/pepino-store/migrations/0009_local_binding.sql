CREATE TABLE local_binding (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    board BLOB NOT NULL CHECK (length(board) = 32),
    identity BLOB NOT NULL CHECK (length(identity) = 32),
    device BLOB NOT NULL CHECK (length(device) = 32)
) STRICT;

CREATE TABLE store_instance (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    id BLOB NOT NULL CHECK (length(id) = 16)
) STRICT;

INSERT INTO store_instance VALUES (1, randomblob(16));

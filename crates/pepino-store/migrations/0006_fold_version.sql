CREATE TABLE fold_version (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    version INTEGER NOT NULL CHECK (version >= 0)
) STRICT;

INSERT INTO fold_version (singleton, version) VALUES (1, 0);

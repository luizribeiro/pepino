CREATE TABLE undecodable (
    board BLOB NOT NULL CHECK (length(board) = 32),
    signer BLOB NOT NULL CHECK (length(signer) = 32),
    bytes BLOB NOT NULL,
    PRIMARY KEY (board, signer, bytes)
) STRICT, WITHOUT ROWID;

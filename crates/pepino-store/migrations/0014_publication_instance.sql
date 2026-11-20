ALTER TABLE publication ADD COLUMN node_instance BLOB CHECK (length(node_instance) = 16);

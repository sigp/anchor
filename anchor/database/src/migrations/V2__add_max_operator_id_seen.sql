ALTER TABLE metadata ADD COLUMN max_operator_id_seen INTEGER;
UPDATE metadata SET schema_version = 2;

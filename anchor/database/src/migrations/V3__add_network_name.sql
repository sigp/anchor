ALTER TABLE metadata ADD COLUMN network_name TEXT;
UPDATE metadata SET schema_version = 3;


PRAGMA foreign_keys=OFF;

BEGIN;
UPDATE metadata SET schema_version = 1;

-- we can not remove a foreign key constraint. so we have to recreate the table instead.
-- see: 7. Making Other Kinds Of Table Schema Changes in https://www.sqlite.org/lang_altertable.html
CREATE TABLE new_cluster_members (
    cluster_id BLOB NOT NULL,
    operator_id INTEGER NOT NULL,
    PRIMARY KEY (cluster_id, operator_id),
    FOREIGN KEY (cluster_id) REFERENCES clusters(cluster_id) ON DELETE CASCADE
);
INSERT INTO new_cluster_members SELECT cluster_id, operator_id FROM cluster_members;
DROP TABLE cluster_members;
ALTER TABLE new_cluster_members RENAME TO cluster_members;

COMMIT;

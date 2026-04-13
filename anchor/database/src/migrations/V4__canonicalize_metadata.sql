-- Rebuild `metadata` so migrated databases and fresh databases converge on the same latest table
-- definition instead of preserving weaker historical defaults and nullability forever.
CREATE TABLE metadata_v4 (
    schema_version INTEGER NOT NULL DEFAULT 4,
    -- `domain_type` is dead data now, but we keep it for compatibility with old code paths and
    -- historical rows.
    domain_type INTEGER NOT NULL DEFAULT 0,
    -- Historical migrations run before startup knows which network this process is opening. The
    -- runtime fills any missing `network_name` value immediately after migrations complete.
    network_name TEXT,
    block_number INTEGER NOT NULL DEFAULT 0 CHECK (block_number >= 0),
    max_operator_id_seen INTEGER DEFAULT 0
);

INSERT INTO metadata_v4 (
    schema_version,
    domain_type,
    network_name,
    block_number,
    max_operator_id_seen
)
SELECT
    4,
    COALESCE(domain_type, 0),
    network_name,
    block_number,
    COALESCE(max_operator_id_seen, 0)
FROM metadata;

DROP TRIGGER unique_metadata;
DROP TABLE metadata;
ALTER TABLE metadata_v4 RENAME TO metadata;

CREATE TRIGGER unique_metadata
    BEFORE INSERT ON metadata
    WHEN (SELECT COUNT(*) FROM metadata) >= 1
BEGIN
    SELECT RAISE(FAIL, 'we can only have one metadata row');
END;

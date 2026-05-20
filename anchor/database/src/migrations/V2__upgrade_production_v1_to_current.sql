-- Fold the unreleased manual schema-v2, v3, and v4 changes into the first refinery-managed
-- upgrade. Keep the shipped schema-v1 metadata table in place and apply the later additive
-- changes exactly the way the manual migration path would have, then let runtime fill the network
-- name and normalize the row values.
ALTER TABLE metadata ADD COLUMN max_operator_id_seen INTEGER;
ALTER TABLE metadata ADD COLUMN network_name TEXT;
CREATE INDEX IF NOT EXISTS idx_validators_validator_index ON validators(validator_index);
CREATE TABLE skipped_operator_adds (
    operator_id INTEGER PRIMARY KEY,
    reason TEXT NOT NULL
);

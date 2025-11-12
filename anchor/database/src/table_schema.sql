-- SCHEMA VERSION 2

-- we should avoid removing columns from this to keep compatibility between anchor Versions
CREATE TABLE metadata (
    schema_version INTEGER NOT NULL DEFAULT 2,
    domain_type INTEGER NOT NULL,
    block_number INTEGER NOT NULL DEFAULT 0 CHECK (block_number >= 0),
    max_operator_id_seen INTEGER DEFAULT 0
);
CREATE TRIGGER unique_metadata
    BEFORE INSERT ON metadata
    WHEN (SELECT COUNT(*) FROM metadata) >= 1
BEGIN
    SELECT RAISE(FAIL, 'we can only have one metadata row');
END;

CREATE TABLE owners (
    owner TEXT PRIMARY KEY NOT NULL,
    fee_recipient TEXT,
    nonce INTEGER
);

CREATE TABLE operators (
    operator_id INTEGER PRIMARY KEY,
    public_key TEXT NOT NULL,
    owner_address TEXT NOT NULL,
    removed BOOLEAN DEFAULT FALSE,
    UNIQUE (public_key)
);

CREATE TABLE clusters (
    cluster_id BLOB PRIMARY KEY,
    owner TEXT NOT NULL,
    liquidated BOOLEAN DEFAULT FALSE
);

CREATE TABLE cluster_members (
    cluster_id BLOB NOT NULL,
    operator_id INTEGER NOT NULL,
    PRIMARY KEY (cluster_id, operator_id),
    FOREIGN KEY (cluster_id) REFERENCES clusters(cluster_id) ON DELETE CASCADE,
    FOREIGN KEY (operator_id) REFERENCES operators(operator_id) ON DELETE RESTRICT -- safeguard, as operators should not be removed while still a member
);

CREATE TABLE validators (
    validator_pubkey TEXT PRIMARY KEY,
    cluster_id BLOB NOT NULL,
    validator_index INTEGER,
    graffiti BLOB DEFAULT X'0000000000000000000000000000000000000000000000000000000000000000',
    FOREIGN KEY (cluster_id) REFERENCES clusters(cluster_id)
);

CREATE TABLE shares (
    validator_pubkey TEXT NOT NULL,
    cluster_id BLOB NOT NULL,
    operator_id INTEGER NOT NULL,
    share_pubkey TEXT,
    encrypted_key BLOB,
    PRIMARY KEY (validator_pubkey, operator_id),
    FOREIGN KEY (cluster_id, operator_id) REFERENCES cluster_members(cluster_id, operator_id) ON DELETE CASCADE,
    FOREIGN KEY (validator_pubkey) REFERENCES validators(validator_pubkey) ON DELETE CASCADE
);

-- Add trigger to clean up empty clusters
CREATE TRIGGER delete_empty_clusters
AFTER DELETE ON validators
WHEN NOT EXISTS (
    SELECT 1 FROM validators
    WHERE cluster_id = OLD.cluster_id
)
BEGIN
    DELETE FROM clusters WHERE cluster_id = OLD.cluster_id;
END;

-- Add triggers to clean up removed operators
CREATE TRIGGER delete_empty_removed_operators_after_delete
    AFTER DELETE ON cluster_members
    WHEN NOT EXISTS (
        SELECT 1 FROM cluster_members
        WHERE operator_id = OLD.operator_id
    )
BEGIN
    DELETE FROM operators WHERE operator_id = OLD.operator_id AND removed = TRUE;
END;

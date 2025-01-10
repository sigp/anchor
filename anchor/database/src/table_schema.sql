CREATE TABLE block (
    block_number INTEGER NOT NULL DEFAULT 0 CHECK (block_number >= 0)
);
INSERT INTO block (block_number) VALUES (0);

CREATE TABLE nonce (
    owner TEXT NOT NULL PRIMARY KEY,
    nonce INTEGER DEFAULT 0
);

CREATE TABLE operators (
    operator_id INTEGER PRIMARY KEY,
    public_key TEXT NOT NULL,
    owner_address TEXT NOT NULL,
    UNIQUE (public_key)
);

CREATE TABLE clusters (
    cluster_id BLOB PRIMARY KEY,
    owner TEXT NOT NULL,
    fee_recipient TEXT NOT NULL,
    faulty INTEGER DEFAULT 0,
    liquidated BOOLEAN DEFAULT FALSE
);

CREATE TABLE cluster_members (
    cluster_id BLOB NOT NULL,
    operator_id INTEGER NOT NULL,
    PRIMARY KEY (cluster_id, operator_id),
    FOREIGN KEY (cluster_id) REFERENCES clusters(cluster_id) ON DELETE CASCADE,
    FOREIGN KEY (operator_id) REFERENCES operators(operator_id) ON DELETE CASCADE
);

CREATE TABLE validators (
    validator_pubkey TEXT PRIMARY KEY,
    cluster_id BLOB NOT NULL,
    validator_index INTEGER DEFAULT 0,
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


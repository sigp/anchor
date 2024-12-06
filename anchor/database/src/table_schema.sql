CREATE TABLE operators (
    operator_id INTEGER PRIMARY KEY,
    public_key TEXT NOT NULL,
    owner_address TEXT NOT NULL,
    UNIQUE (public_key)
);

CREATE TABLE clusters (
    cluster_id INTEGER PRIMARY KEY,
    faulty INTEGER NOT NULL,
    liquidated BOOLEAN DEFAULT FALSE
);

CREATE TABLE cluster_members (
    cluster_id INTEGER NOT NULL,
    operator_id INTEGER NOT NULL,
    PRIMARY KEY (cluster_id, operator_id),
    FOREIGN KEY (cluster_id) REFERENCES clusters(cluster_id) ON DELETE CASCADE,
    FOREIGN KEY (operator_id) REFERENCES operators(operator_id) ON DELETE CASCADE
);

CREATE TABLE validators (
    validator_pubkey TEXT PRIMARY KEY,
    cluster_id INTEGER NOT NULL,
    fee_recipient TEXT,
    graffiti BLOB,
    validator_index INTEGER,
    last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (cluster_id) REFERENCES clusters(cluster_id) ON DELETE CASCADE
);

CREATE TABLE shares (
    validator_pubkey TEXT NOT NULL,
    cluster_id INTEGER NOT NULL,
    operator_id INTEGER NOT NULL,
    share_pubkey TEXT,
    PRIMARY KEY (validator_pubkey, operator_id),
    FOREIGN KEY (cluster_id, operator_id) REFERENCES cluster_members(cluster_id, operator_id) ON DELETE CASCADE,
    FOREIGN KEY (validator_pubkey) REFERENCES validators(validator_pubkey) ON DELETE CASCADE
);


-- First post-cutover migration: match the unreleased manual schema v4 change.
CREATE INDEX IF NOT EXISTS idx_validators_validator_index ON validators(validator_index);

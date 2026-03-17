use std::{str::FromStr, sync::Arc};

use alloy::primitives::{Address, Bytes};
use bls::PublicKeyBytes;
use database::SlashingProtection;

mod common;

use common::*;

struct FailingSlashingProtection;

impl SlashingProtection for FailingSlashingProtection {
    fn register_validator(&self, _public_key: PublicKeyBytes) -> Result<(), String> {
        Err("boom".to_string())
    }
}

fn add_operators(test: &ProcessorFixture, operator_ids: &[u64], block_number: u64) {
    let logs = operator_ids
        .iter()
        .enumerate()
        .map(|(index, operator_id)| {
            let mut log = create_operator_added_log(
                *operator_id,
                Address::random(),
                create_valid_rsa_public_key_bytes(),
                1000,
            );
            log.block_number = Some(block_number);
            log.log_index = Some(index as u64);
            log
        })
        .collect();

    test.processor
        .process_logs(logs, true, block_number)
        .expect("Operator setup should succeed");
}

#[tokio::test]
async fn test_validator_added_event_processing() {
    setup_tracing();

    let mut test = ProcessorFixture::new();
    let operator_ids = test.get_operator_ids();
    let owner = test.cluster.owner;

    let (shares, validator_pubkey_bytes) =
        create_valid_shares_data_for_owner_and_nonce(&operator_ids, owner, 1);
    let public_key = Bytes::from(validator_pubkey_bytes.serialize().to_vec());

    let log = create_validator_added_log(owner, operator_ids, public_key, shares);

    let result = test.processor.process_logs(vec![log], true, 12346);
    assert!(
        result.is_ok(),
        "ValidatorAdded should be processed successfully with valid signature"
    );

    let queued_pubkey = test
        .index_sync_rx
        .recv()
        .await
        .expect("Validator should be queued for index sync");
    assert_eq!(queued_pubkey, validator_pubkey_bytes);
}

#[tokio::test]
async fn test_duplicate_validator_added_is_skipped() {
    setup_tracing();

    let mut test = ProcessorFixture::new_empty();
    let owner = Address::from_str(TEST_CLUSTER_OWNER).expect("Invalid address");

    let operator_ids = vec![1u64, 2, 3, 4];
    add_operators(&test, &operator_ids, 100);

    let validator_secret_key = bls::SecretKey::random();
    let (first_shares, validator_pubkey) = create_valid_shares_data_for_validator_and_owner_nonce(
        &operator_ids,
        owner,
        0,
        &validator_secret_key,
    );
    let mut first_log = create_validator_added_log(
        owner,
        operator_ids.clone(),
        Bytes::from(validator_pubkey.serialize().to_vec()),
        first_shares,
    );
    first_log.block_number = Some(101);
    first_log.log_index = Some(0);

    test.processor
        .process_logs(vec![first_log], true, 101)
        .expect("Initial validator addition should succeed");

    let queued_pubkey = test
        .index_sync_rx
        .recv()
        .await
        .expect("Validator should be queued for index sync");
    assert_eq!(queued_pubkey, validator_pubkey);

    let (duplicate_shares, _) = create_valid_shares_data_for_validator_and_owner_nonce(
        &operator_ids,
        owner,
        1,
        &validator_secret_key,
    );
    let mut duplicate_log = create_validator_added_log(
        owner,
        operator_ids,
        Bytes::from(validator_pubkey.serialize().to_vec()),
        duplicate_shares,
    );
    duplicate_log.block_number = Some(102);
    duplicate_log.log_index = Some(0);

    test.processor
        .process_logs(vec![duplicate_log], true, 102)
        .expect("Duplicate validator addition should be skipped");

    assert_eq!(test.processor.db.state().metadata().length(), 1);
    assert_eq!(test.processor.db.state().get_last_processed_block(), 102);
    assert_eq!(test.processor.db.state().get_next_nonce(&owner), 2);
    assert!(
        test.index_sync_rx.try_recv().is_err(),
        "Duplicate validator should not be queued for index sync again"
    );
}

#[tokio::test]
async fn test_validator_added_with_missing_operators_still_bumps_nonce() {
    setup_tracing();

    let mut test = ProcessorFixture::new_empty();
    let owner = Address::random();
    let operator_ids = vec![1u64, 2, 3, 4];

    let (shares, validator_pubkey) =
        create_valid_shares_data_for_owner_and_nonce(&operator_ids, owner, 0);
    let log = create_validator_added_log(
        owner,
        operator_ids,
        Bytes::from(validator_pubkey.serialize().to_vec()),
        shares,
    );

    test.processor
        .process_logs(vec![log], true, 12360)
        .expect("Missing operators should skip the validator while preserving nonce progress");

    assert_eq!(test.processor.db.state().metadata().length(), 0);
    assert_eq!(test.processor.db.state().get_next_nonce(&owner), 1);
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12360);
    assert!(test.index_sync_rx.try_recv().is_err());
}

#[tokio::test]
async fn test_invalid_validator_added_shares_still_bump_nonce() {
    setup_tracing();

    let mut test = ProcessorFixture::new();
    let owner = test.cluster.owner;
    let operator_ids = test.get_operator_ids();
    let starting_nonce = test.processor.db.state().get_next_nonce(&owner);
    let starting_validators = test.processor.db.state().metadata().length();

    let (_, validator_pubkey) =
        create_valid_shares_data_for_owner_and_nonce(&operator_ids, owner, starting_nonce);
    let log = create_validator_added_log(
        owner,
        operator_ids,
        Bytes::from(validator_pubkey.serialize().to_vec()),
        Bytes::from_static(b"invalid-shares"),
    );

    test.processor
        .process_logs(vec![log], true, 12361)
        .expect("Malformed validator data should be skipped after committing nonce progress");

    assert_eq!(
        test.processor.db.state().metadata().length(),
        starting_validators
    );
    assert_eq!(
        test.processor.db.state().get_next_nonce(&owner),
        starting_nonce + 1
    );
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12361);
    assert!(test.index_sync_rx.try_recv().is_err());
}

#[tokio::test]
async fn test_keysplit_validator_added_only_bumps_nonce() {
    use database::test_utils::{TEST_NETWORK, generators};

    setup_tracing();

    let pubkey = generators::pubkey::random_rsa();
    let db = Arc::new(
        database::NetworkDatabase::new_in_memory(&pubkey, TEST_NETWORK)
            .expect("Failed to create in-memory database"),
    );
    let processor = create_keysplit_mode_processor(Arc::clone(&db));

    let owner = Address::random();
    let operator_ids = vec![1u64, 2, 3, 4];
    let (shares, validator_pubkey) =
        create_valid_shares_data_for_owner_and_nonce(&operator_ids, owner, 0);
    let log = create_validator_added_log(
        owner,
        operator_ids,
        Bytes::from(validator_pubkey.serialize().to_vec()),
        shares,
    );

    processor
        .process_logs(vec![log], true, 12362)
        .expect("KeySplit mode should only commit nonce progress for ValidatorAdded");

    assert_eq!(db.state().metadata().length(), 0);
    assert_eq!(db.state().get_next_nonce(&owner), 1);
    assert_eq!(db.state().get_last_processed_block(), 12362);
}

#[tokio::test]
async fn test_slashing_registration_failure_aborts_validator_added() {
    setup_tracing();

    let mut test = ProcessorFixture::new_empty_with_slashing(Arc::new(FailingSlashingProtection));
    let owner = Address::from_str(TEST_CLUSTER_OWNER).expect("Invalid address");
    let operator_ids = vec![1u64, 2, 3, 4];

    add_operators(&test, &operator_ids, 110);

    let (shares, validator_pubkey) =
        create_valid_shares_data_for_owner_and_nonce(&operator_ids, owner, 0);
    let log = create_validator_added_log(
        owner,
        operator_ids,
        Bytes::from(validator_pubkey.serialize().to_vec()),
        shares,
    );

    let result = test.processor.process_logs(vec![log], true, 12363);
    assert!(result.is_err(), "Slashing registration failure must abort processing");

    assert_eq!(test.processor.db.state().metadata().length(), 0);
    assert_eq!(test.processor.db.state().get_next_nonce(&owner), 0);
    assert_eq!(test.processor.db.state().get_last_processed_block(), 110);
    assert_eq!(test.processor.db.state().get_last_processed_event(), None);
    assert!(test.index_sync_rx.try_recv().is_err());
}

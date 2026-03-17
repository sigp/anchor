use std::collections::HashMap;

use alloy::primitives::{Address, Bytes};
use ssv_types::ValidatorIndex;

mod common;

use common::*;

async fn setup_validator_via_events(
    with_index: bool,
) -> (ProcessorFixture, Address, Vec<u64>, bls::PublicKeyBytes) {
    let mut test = ProcessorFixture::new_empty();
    let owner = Address::random();
    let operator_ids = vec![1u64, 2, 3, 4];

    let operator_logs = operator_ids
        .iter()
        .enumerate()
        .map(|(index, operator_id)| {
            let mut log = create_operator_added_log(
                *operator_id,
                Address::random(),
                create_valid_rsa_public_key_bytes(),
                1000,
            );
            log.block_number = Some(120);
            log.log_index = Some(index as u64);
            log
        })
        .collect();
    test.processor
        .process_logs(operator_logs, true, 120)
        .expect("Operator setup should succeed");

    let (shares, validator_pubkey) =
        create_valid_shares_data_for_owner_and_nonce(&operator_ids, owner, 0);
    let validator_log = create_validator_added_log(
        owner,
        operator_ids.clone(),
        Bytes::from(validator_pubkey.serialize().to_vec()),
        shares,
    );
    test.processor
        .process_logs(vec![validator_log], true, 121)
        .expect("Validator setup should succeed");
    let _ = test.index_sync_rx.recv().await;

    if with_index {
        test.processor
            .db
            .set_validator_indices(HashMap::from([(validator_pubkey, ValidatorIndex(42))]))
            .expect("Setting validator index should succeed");
    }

    (test, owner, operator_ids, validator_pubkey)
}

#[tokio::test]
async fn test_live_validator_exited_queues_work_and_advances_progress() {
    setup_tracing();

    let (mut test, owner, operator_ids, validator_pubkey) = setup_validator_via_events(true).await;
    let public_key = Bytes::from(validator_pubkey.serialize().to_vec());
    let log = create_validator_exited_log(owner, operator_ids, public_key);

    test.processor
        .process_logs(vec![log], true, 12380)
        .expect("Live validator exits should queue work successfully");

    let exit_request = test
        .exit_rx
        .recv()
        .await
        .expect("Validator exit should be queued");
    assert_eq!(exit_request.validator_pubkey, validator_pubkey);
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12380);
    assert_eq!(test.processor.db.state().get_last_processed_event(), None);
}

#[tokio::test]
async fn test_historic_validator_exited_is_ignored_but_progress_advances() {
    setup_tracing();

    let (mut test, owner, operator_ids, validator_pubkey) = setup_validator_via_events(true).await;
    let public_key = Bytes::from(validator_pubkey.serialize().to_vec());
    let log = create_validator_exited_log(owner, operator_ids, public_key);

    test.processor
        .process_logs(vec![log], false, 12381)
        .expect("Historic validator exits should be ignored successfully");

    assert!(test.exit_rx.try_recv().is_err());
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12381);
    assert_eq!(test.processor.db.state().get_last_processed_event(), None);
}

#[tokio::test]
async fn test_live_validator_exited_without_index_advances_progress_without_queueing() {
    setup_tracing();

    let (mut test, owner, operator_ids, validator_pubkey) = setup_validator_via_events(false).await;
    let exit_log = create_validator_exited_log(
        owner,
        operator_ids,
        Bytes::from(validator_pubkey.serialize().to_vec()),
    );
    test.processor
        .process_logs(vec![exit_log], true, 12382)
        .expect("Validators without an index should be ignored after recording progress");

    assert!(test.exit_rx.try_recv().is_err());
    assert_eq!(test.processor.db.state().get_last_processed_block(), 12382);
    assert_eq!(test.processor.db.state().get_last_processed_event(), None);
}

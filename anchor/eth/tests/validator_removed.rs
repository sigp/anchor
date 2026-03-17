use alloy::primitives::Bytes;

mod common;

use common::*;

#[tokio::test]
async fn test_validator_removed_missing_state_is_fatal() {
    setup_tracing();

    let test = ProcessorFixture::new_empty();
    let owner = test.cluster.owner;
    let operator_ids = vec![1u64, 2, 3, 4];
    let public_key = Bytes::from(test.validator.public_key.serialize().to_vec());
    let log = create_validator_removed_log(owner, operator_ids, public_key);

    let result = test.processor.process_logs(vec![log], true, 12370);
    assert!(
        result.is_err(),
        "Missing committed validator state should remain a fatal inconsistency"
    );

    assert_eq!(test.processor.db.state().get_last_processed_block(), 0);
    assert_eq!(test.processor.db.state().get_last_processed_event(), None);
}

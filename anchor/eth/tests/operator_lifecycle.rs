use std::str::FromStr;

use alloy::primitives::{Address, Bytes};
use ssv_types::*;

mod common;

use common::*;

/// Tests the complete operator lifecycle in the SSV network across multiple clusters.
///
/// This test validates that:
/// 1. Operators can be added and participate in multiple clusters
/// 2. When an operator is removed while still participating in active clusters, it gets soft
///    deleted (removed from memory but record remains in database for cluster integrity)
/// 3. When clusters are removed one by one, the operator should only be hard deleted when the last
///    cluster referencing it is removed
/// 4. Other operators not marked for removal remain unaffected throughout the process
///
/// **Technical Details:**
/// - Uses cryptographically valid shares data with proper BLS signature verification
/// - Tests the multi-cluster operator lifecycle scenario
/// - Validates both database state and in-memory state consistency
#[tokio::test]
async fn test_operator_lifecycle_soft_delete_behavior() {
    setup_tracing();

    // Setup test fixture with processor
    let test = ProcessorFixture::new_empty();

    // Create 5 operators to enable different cluster combinations
    // IDs (1,2,3,4) match the cryptographic shares data for signature verification
    let operator_ids = vec![1u64, 2u64, 3u64, 4u64, 5u64];
    let owners: Vec<Address> = (0..5).map(|_| Address::random()).collect();
    let public_keys: Vec<Bytes> = (0..5)
        .map(|_| create_valid_rsa_public_key_bytes())
        .collect();

    // Add all operators
    let mut logs = Vec::new();
    for i in 0..5 {
        let log = create_operator_added_log(
            operator_ids[i],
            owners[i],
            public_keys[i].clone(),
            1000 + i as u64 * 100,
        );
        logs.push(log);
    }

    // Process operator additions
    let result = test.processor.process_logs(logs, true, 12345);
    assert!(result.is_ok(), "Adding operators should succeed");

    // Verify all operators exist
    for &operator_id in &operator_ids {
        verify_operator_stored(&test.processor, OperatorId(operator_id));
    }

    // Create a cluster with all 4 operators using ValidatorAdded event
    let cluster_owner = Address::from_str(TEST_CLUSTER_OWNER).expect("Invalid address");

    let cluster1_operators = operator_ids[..4].to_vec(); // First 4 operators [1,2,3,4]

    // Generate valid shares data with matching signature for the first cluster
    let (cluster1_shares, validator1_pubkey_bytes) = create_valid_shares_data_for_owner_and_nonce(
        &cluster1_operators,
        cluster_owner,
        0, // Use nonce 0 for first cluster
    );
    let validator_public_key = Bytes::from(validator1_pubkey_bytes.serialize().to_vec());

    let validator_log = create_validator_added_log(
        cluster_owner,
        cluster1_operators.clone(),
        validator_public_key.clone(),
        cluster1_shares,
    );
    let result = test
        .processor
        .process_logs(vec![validator_log], true, 12346);
    assert!(
        result.is_ok(),
        "Adding validator should succeed - signature verification should pass"
    );

    // CRITICAL VERIFICATION: Ensure the validator and cluster were actually created
    // This is essential because the soft delete behavior only occurs when operators are part of
    // active clusters
    let validator_pubkey_str = &format!("0x{}", hex::encode(validator1_pubkey_bytes.serialize()));
    verify_validator_added(&test.processor, validator_pubkey_str);
    verify_cluster_created(&test.processor, cluster_owner, &cluster1_operators);

    // Create second cluster with different operator set (different operators = different cluster)
    let cluster2_operators = vec![
        operator_ids[0],
        operator_ids[1],
        operator_ids[2],
        operator_ids[4],
    ]; // [1,2,3,5] - different from first cluster [1,2,3,4]

    // Generate valid shares data with matching signature for the second cluster
    let (cluster2_shares, validator2_pubkey_bytes) = create_valid_shares_data_for_owner_and_nonce(
        &cluster2_operators,
        cluster_owner,
        1, // Different nonce to ensure different signature
    );
    let validator2_pubkey = Bytes::from(validator2_pubkey_bytes.serialize().to_vec());

    let validator2_log = create_validator_added_log(
        cluster_owner, // Same owner as first cluster
        cluster2_operators.clone(),
        validator2_pubkey.clone(),
        cluster2_shares,
    );
    let result = test
        .processor
        .process_logs(vec![validator2_log], true, 12347);
    assert!(
        result.is_ok(),
        "Adding validator to second cluster should succeed"
    );

    // Verify second cluster was created
    verify_cluster_created(&test.processor, cluster_owner, &cluster2_operators);

    // Remove first operator (should be deleted from memory but soft deleted in database)
    let operator_to_remove = OperatorId(operator_ids[0]); // Remove first operator (ID=1)
    let removal_log = create_operator_removed_log(operator_ids[0]);
    let result = test.processor.process_logs(vec![removal_log], true, 12348);
    assert!(result.is_ok(), "Removing operator should succeed");

    // Operator should be soft deleted (removed from memory but record remains in database)
    verify_operator_soft_deleted(&test.processor, operator_to_remove);

    // Verify other operators still exist normally
    for &operator_id in &operator_ids[1..] {
        verify_operator_stored(&test.processor, OperatorId(operator_id));
    }

    // Remove first cluster
    let validator1_removal_log =
        create_validator_removed_log(cluster_owner, cluster1_operators, validator_public_key);
    let result = test
        .processor
        .process_logs(vec![validator1_removal_log], true, 12349);
    assert!(result.is_ok(), "Removing first cluster should succeed");

    // Operator should still be soft deleted (still referenced by second cluster)
    verify_operator_soft_deleted(&test.processor, operator_to_remove);

    // Verify other operators still exist normally
    for &operator_id in &operator_ids[1..] {
        verify_operator_stored(&test.processor, OperatorId(operator_id));
    }

    // Remove second cluster (last cluster containing the operator)
    let validator2_removal_log =
        create_validator_removed_log(cluster_owner, cluster2_operators, validator2_pubkey);
    let result = test
        .processor
        .process_logs(vec![validator2_removal_log], true, 12350);
    assert!(result.is_ok(), "Removing second cluster should succeed");

    // Now operator should be hard deleted since no clusters reference it
    verify_operator_hard_deleted(&test.processor, operator_to_remove);

    // Verify other operators still exist (they were not marked as removed, so they should remain)
    for &operator_id in &operator_ids[1..] {
        verify_operator_stored(&test.processor, OperatorId(operator_id));
    }
}

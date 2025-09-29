use std::{str::FromStr, sync::Arc};

use alloy::primitives::{Address, Bytes};
use database::test_utils::TestFixture;
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

    // Setup test fixture with empty database
    let fixture = TestFixture::new_empty();
    let (processor, _index_sync_rx) = create_node_mode_processor(Arc::new(fixture.db));

    // Create 4 operators with the same IDs as in the VALID_SHARES_DATA
    // These IDs (1,2,3,4) match the cryptographic shares data for signature verification
    let operator_ids = vec![1u64, 2u64, 3u64, 4u64];
    let owners: Vec<Address> = (0..4).map(|_| Address::random()).collect();
    let public_keys: Vec<Bytes> = (0..4)
        .map(|_| create_valid_rsa_public_key_bytes())
        .collect();

    // Add all operators
    let mut logs = Vec::new();
    for i in 0..4 {
        let log = create_operator_added_log(
            operator_ids[i],
            owners[i],
            public_keys[i].clone(),
            1000 + i as u64 * 100,
        );
        logs.push(log);
    }

    // Process operator additions
    let result = processor.process_logs(logs, true, 12345);
    assert!(result.is_ok(), "Adding operators should succeed");

    // Verify all operators exist
    for &operator_id in &operator_ids {
        verify_operator_stored(&processor, OperatorId(operator_id));
    }

    // Create a cluster with all 4 operators using ValidatorAdded event
    // CRITICAL: Use the exact same values as in VALID_SHARES_DATA for signature verification
    // The shares data contains a BLS signature over "owner:nonce" that must match exactly
    let cluster_owner =
        Address::from_str("0x000000633b68f5d8d3a86593ebb815b4663bcbe0").expect("Invalid address");
    let shares_data = hex::decode(VALID_SHARES_DATA).expect("Failed to decode hex string");
    let shares = Bytes::from(shares_data.clone());
    let validator_public_key = Bytes::from_str("0x97e8235ec2174862a8162ef9624f2fb1df82a3a8ef57f72a2a866df37c3da66020b1e4070d0d443ef40198e71afe9493").expect("Invalid public key");

    let validator_log = create_validator_added_log(
        cluster_owner,
        operator_ids.clone(),
        validator_public_key.clone(),
        shares,
    );
    let result = processor.process_logs(vec![validator_log], true, 12346);
    assert!(
        result.is_ok(),
        "Adding validator should succeed - signature verification should pass"
    );

    // CRITICAL VERIFICATION: Ensure the validator and cluster were actually created
    // This is essential because the soft delete behavior only occurs when operators are part of
    // active clusters
    let validator_pubkey_str = "0x97e8235ec2174862a8162ef9624f2fb1df82a3a8ef57f72a2a866df37c3da66020b1e4070d0d443ef40198e71afe9493";
    verify_validator_added(&processor, validator_pubkey_str);
    verify_cluster_created(&processor, cluster_owner, &operator_ids);

    // Create second cluster with different owner (different owner = different cluster)
    let cluster2_owner =
        Address::from_str("0x111111633b68f5d8d3a86593ebb815b4663bcbe1").expect("Invalid address");
    let cluster2_operators = operator_ids.clone(); // Use same 4 operators but different owner
    let validator2_pubkey = Bytes::from_str("0x88f77c9d6280e1b1c5e0c7b4c9a8d5e3f1b2c4d6e8f0a2b4c6d8e0f2a4b6c8d0e2f4a6b8c0d2e4f6a8b0c2d4e6f8a0b2").expect("Invalid public key");

    let validator2_log = create_validator_added_log(
        cluster2_owner,
        cluster2_operators.clone(),
        validator2_pubkey.clone(),
        Bytes::from(shares_data.clone()),
    );
    let result = processor.process_logs(vec![validator2_log], true, 12347);
    assert!(
        result.is_ok(),
        "Adding validator to second cluster should succeed"
    );

    // Remove first operator (should be deleted from memory but soft deleted in database)
    let operator_to_remove = OperatorId(operator_ids[0]); // Remove first operator (ID=1)
    let removal_log = create_operator_removed_log(operator_ids[0]);
    let result = processor.process_logs(vec![removal_log], true, 12348);
    assert!(result.is_ok(), "Removing operator should succeed");

    // Operator should be soft deleted (removed from memory but record remains in database)
    verify_operator_soft_deleted(&processor, operator_to_remove);

    // Verify other operators still exist normally
    for &operator_id in &operator_ids[1..] {
        verify_operator_stored(&processor, OperatorId(operator_id));
    }

    // Remove first cluster
    let validator1_removal_log =
        create_validator_removed_log(cluster_owner, operator_ids.clone(), validator_public_key);
    let result = processor.process_logs(vec![validator1_removal_log], true, 12349);
    assert!(result.is_ok(), "Removing first cluster should succeed");

    // Operator should still be soft deleted (still referenced by second cluster)
    verify_operator_soft_deleted(&processor, operator_to_remove);

    // Verify other operators still exist normally
    for &operator_id in &operator_ids[1..] {
        verify_operator_stored(&processor, OperatorId(operator_id));
    }

    // Remove second cluster (last cluster containing the operator)
    let validator2_removal_log =
        create_validator_removed_log(cluster2_owner, cluster2_operators, validator2_pubkey);
    let result = processor.process_logs(vec![validator2_removal_log], true, 12350);
    assert!(result.is_ok(), "Removing second cluster should succeed");

    // Now operator should be hard deleted since no clusters reference it
    verify_operator_hard_deleted(&processor, operator_to_remove);

    // Verify other operators still exist (they were not marked as removed, so they should remain)
    for &operator_id in &operator_ids[1..] {
        verify_operator_stored(&processor, OperatorId(operator_id));
    }
}

use std::{str::FromStr, sync::Arc};

use alloy::primitives::{Address, Bytes};
use database::test_utils::TestFixture;
use ssv_types::*;

mod common;

use common::*;

/// Tests the complete operator lifecycle in the SSV network, focusing on the soft delete -> hard
/// delete behavior.
///
/// This test validates that:
/// 1. Operators can be added and participate in clusters
/// 2. When an operator is removed while still participating in active clusters, it gets "soft
///    deleted" (marked as removed=TRUE in database but record remains for cluster integrity)
/// 3. When the last cluster using a soft-deleted operator is removed, the operator gets "hard
///    deleted" (completely removed from database via SQL trigger)
/// 4. Other operators not marked for removal remain unaffected throughout the process
///
/// **Technical Details:**
/// - Uses cryptographically valid shares data with proper BLS signature verification
/// - Tests actual production database triggers and foreign key constraints
/// - Validates both database state and in-memory state consistency
/// - Uses the same signature scheme as real SSV network (owner:nonce hash verification)
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
    let shares = Bytes::from(shares_data);
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

    // Phase 1: Remove one operator (should be soft deleted since it's still in a cluster)
    let operator_to_remove = OperatorId(operator_ids[0]); // Remove first operator (ID=1)
    let removal_log = create_operator_removed_log(operator_ids[0]);
    let result = processor.process_logs(vec![removal_log], true, 12347);
    assert!(result.is_ok(), "Removing operator should succeed");

    // Verify operator is soft deleted (removed=TRUE in database but record exists)
    // This is critical for cluster integrity - the operator record must remain while clusters
    // reference it
    verify_operator_soft_deleted(&processor, operator_to_remove);

    // Verify other operators still exist normally
    for &operator_id in &operator_ids[1..] {
        verify_operator_stored(&processor, OperatorId(operator_id));
    }

    // Phase 2: Remove the validator (this should trigger cluster cleanup and hard delete of the
    // removed operator) The SQL trigger should detect that operator ID=1 is marked removed=TRUE
    // and has no more cluster references
    let validator_removal_log =
        create_validator_removed_log(cluster_owner, operator_ids.clone(), validator_public_key);
    let result = processor.process_logs(vec![validator_removal_log], true, 12348);
    assert!(result.is_ok(), "Removing validator should succeed");

    // Verify the removed operator is now hard deleted (completely removed from database)
    // This validates that the SQL trigger properly cleaned up the soft-deleted operator
    verify_operator_hard_deleted(&processor, operator_to_remove);

    // Verify other operators still exist (they were not marked as removed, so they should remain)
    for &operator_id in &operator_ids[1..] {
        verify_operator_stored(&processor, OperatorId(operator_id));
    }
}

use std::{collections::HashMap, sync::Arc};

use bls::{PublicKeyBytes, SecretKey, Signature};
use bls_lagrange::{KeyId, split_with_rng};
use database::{NetworkDatabase, test_utils::generators};
use rand::{prelude::*, rngs::StdRng};
use ssv_types::{ENCRYPTED_KEY_LENGTH, Share, ValidatorMetadata};
use tokio::sync::{mpsc, oneshot};
use types::{Graffiti, Hash256};

use super::*;

/// RNG seed for deterministic test key generation.
const TEST_RNG_SEED: u64 = 0xDEAD_BEEF_CAFE_0001;

/// Number of total shares to split the master key into.
const TOTAL_SHARES: u64 = 4;

/// Threshold required to reconstruct the master signature.
const THRESHOLD: u64 = 3;

/// Signing root used across all tests (arbitrary 32-byte value).
const SIGNING_ROOT_BYTES: [u8; 32] = [0xAB; 32];

/// Holds all cryptographic material needed by the tests.
struct TestKeyMaterial {
    master_pubkey_bytes: PublicKeyBytes,
    /// (`OperatorId`, `SecretKey`) pairs for each share.
    shares: Vec<(OperatorId, SecretKey)>,
    signing_root: Hash256,
}

/// Builds a second, independent set of shares whose signatures will fail
/// verification against the original share pubkeys.
struct GarbageKeyMaterial {
    shares: Vec<(OperatorId, SecretKey)>,
}

/// Creates deterministic key material: a master key split into `TOTAL_SHARES`
/// shares with `THRESHOLD` threshold.
fn create_test_key_material() -> TestKeyMaterial {
    let rng = &mut StdRng::seed_from_u64(TEST_RNG_SEED);

    let master = SecretKey::random();
    let master_pubkey_bytes = PublicKeyBytes::from(master.public_key());

    let split_keys = split_with_rng(
        &master,
        THRESHOLD,
        (1..=TOTAL_SHARES).map(|x| KeyId::try_from(x).unwrap()),
        rng,
    )
    .expect("split should succeed");

    let shares: Vec<(OperatorId, SecretKey)> = split_keys
        .into_iter()
        .map(|(kid, sk)| {
            let op_id = OperatorId(u64::from(kid));
            (op_id, sk)
        })
        .collect();

    let signing_root = Hash256::from(SIGNING_ROOT_BYTES);

    TestKeyMaterial {
        master_pubkey_bytes,
        shares,
        signing_root,
    }
}

/// Creates a second independent master key and splits it, producing shares
/// whose signatures are valid BLS signatures but will not verify against the
/// original share pubkeys.
fn create_garbage_key_material() -> GarbageKeyMaterial {
    let rng = &mut StdRng::seed_from_u64(TEST_RNG_SEED ^ 0xFFFF_FFFF);

    let garbage_master = SecretKey::random();

    let split_keys = split_with_rng(
        &garbage_master,
        THRESHOLD,
        (1..=TOTAL_SHARES).map(|x| KeyId::try_from(x).unwrap()),
        rng,
    )
    .expect("garbage split should succeed");

    let shares: Vec<(OperatorId, SecretKey)> = split_keys
        .into_iter()
        .map(|(kid, sk)| {
            let op_id = OperatorId(u64::from(kid));
            (op_id, sk)
        })
        .collect();

    GarbageKeyMaterial { shares }
}

/// Builds a `share_pubkeys` map from the test key material (`OperatorId` -> share public key
/// bytes).
fn build_share_pubkey_map(material: &TestKeyMaterial) -> HashMap<OperatorId, PublicKeyBytes> {
    material
        .shares
        .iter()
        .map(|(op_id, sk)| (*op_id, PublicKeyBytes::from(sk.public_key())))
        .collect()
}

/// Builds a `HashMap<OperatorId, Signature>` by signing the given root with each share.
fn sign_shares(
    shares: &[(OperatorId, SecretKey)],
    signing_root: Hash256,
) -> HashMap<OperatorId, Signature> {
    shares
        .iter()
        .map(|(op_id, sk)| (*op_id, sk.sign(signing_root)))
        .collect()
}

// ==================== `find_invalid_shares` tests ====================

/// Three valid shares and one garbage share. The function should identify
/// exactly the garbage operator as invalid.
#[test]
fn find_invalid_shares_identifies_bad() {
    // Arrange
    let material = create_test_key_material();
    let garbage = create_garbage_key_material();
    let share_pubkeys = build_share_pubkey_map(&material);

    // Sign the first 3 shares with the correct keys
    let mut sig_map: HashMap<OperatorId, Signature> = material.shares[..3]
        .iter()
        .map(|(op, sk)| (*op, sk.sign(material.signing_root)))
        .collect();

    // Sign the 4th share with the garbage key (same operator ID, wrong key)
    let bad_op_id = material.shares[3].0;
    let garbage_sig = garbage.shares[3].1.sign(material.signing_root);
    sig_map.insert(bad_op_id, garbage_sig);

    // Act
    let invalid = find_invalid_shares(&sig_map, material.signing_root, &share_pubkeys);

    // Assert
    assert_eq!(
        invalid.len(),
        1,
        "Expected exactly 1 invalid share, got {}",
        invalid.len()
    );
    assert!(
        invalid.contains(&bad_op_id),
        "Expected operator {:?} to be flagged as invalid",
        bad_op_id
    );
}

/// All three shares are valid. The returned set should be empty.
#[test]
fn find_invalid_shares_all_valid() {
    // Arrange
    let material = create_test_key_material();
    let share_pubkeys = build_share_pubkey_map(&material);
    let sig_map = sign_shares(&material.shares[..3], material.signing_root);

    // Act
    let invalid = find_invalid_shares(&sig_map, material.signing_root, &share_pubkeys);

    // Assert
    assert!(
        invalid.is_empty(),
        "Expected no invalid shares, but got {:?}",
        invalid
    );
}

/// An operator is present in the shares map but has no corresponding entry in
/// the pubkey map. It should be marked as invalid.
#[test]
fn find_invalid_shares_missing_pubkey() {
    // Arrange
    let material = create_test_key_material();
    let sig_map = sign_shares(&material.shares[..3], material.signing_root);

    // Build a pubkey map that is missing the first operator
    let missing_op = material.shares[0].0;
    let mut share_pubkeys = build_share_pubkey_map(&material);
    share_pubkeys.remove(&missing_op);

    // Act
    let invalid = find_invalid_shares(&sig_map, material.signing_root, &share_pubkeys);

    // Assert
    assert!(
        invalid.contains(&missing_op),
        "Expected operator {:?} with missing pubkey to be flagged as invalid",
        missing_op
    );
}

// ==================== `try_combine_and_verify tests` ====================

/// Three valid shares at threshold 3 should combine and verify successfully
/// against the master public key.
#[test]
fn try_combine_valid() {
    // Arrange
    let material = create_test_key_material();
    let sig_map = sign_shares(&material.shares[..3], material.signing_root);

    // Act
    let outcome = try_combine_and_verify(
        &sig_map,
        &material.master_pubkey_bytes,
        material.signing_root,
    );

    // Assert
    assert!(
        matches!(outcome, CombineOutcome::Success(_)),
        "Expected CombineOutcome::Success, got {:?}",
        match &outcome {
            CombineOutcome::Success(_) => "Success",
            CombineOutcome::CombineFailed(_) => "CombineFailed",
            CombineOutcome::VerificationFailed => "VerificationFailed",
        }
    );
}

/// Two valid shares plus one garbage share at threshold 3 should produce a
/// reconstructed signature that fails verification against the master pubkey.
#[test]
fn try_combine_poisoned() {
    // Arrange
    let material = create_test_key_material();
    let garbage = create_garbage_key_material();

    // First 2 valid shares
    let mut sig_map: HashMap<OperatorId, Signature> = material.shares[..2]
        .iter()
        .map(|(op, sk)| (*op, sk.sign(material.signing_root)))
        .collect();

    // Third share from garbage key material (valid BLS sig, wrong key)
    let poisoned_op = material.shares[2].0;
    let garbage_sig = garbage.shares[2].1.sign(material.signing_root);
    sig_map.insert(poisoned_op, garbage_sig);

    // Act
    let outcome = try_combine_and_verify(
        &sig_map,
        &material.master_pubkey_bytes,
        material.signing_root,
    );

    // Assert
    assert!(
        matches!(outcome, CombineOutcome::VerificationFailed),
        "Expected CombineOutcome::VerificationFailed when one share is garbage"
    );
}

// ==================== resolve_duplicate_signature tests ====================

/// Old signature is valid, new signature is garbage. The old signature should
/// be retained in the map.
#[test]
fn resolve_duplicate_keeps_valid() {
    // Arrange
    let material = create_test_key_material();
    let garbage = create_garbage_key_material();
    let share_pubkeys = build_share_pubkey_map(&material);

    let target_op = material.shares[0].0;
    let valid_sig = material.shares[0].1.sign(material.signing_root);
    let garbage_sig = garbage.shares[0].1.sign(material.signing_root);

    let mut shares = HashMap::new();
    shares.insert(target_op, valid_sig.clone());

    // Act
    resolve_duplicate_signature(
        &mut shares,
        target_op,
        &garbage_sig,
        material.signing_root,
        &share_pubkeys,
    );

    // Assert
    assert!(
        shares.contains_key(&target_op),
        "Map should still contain the operator after duplicate resolution"
    );
    assert_eq!(
        shares.get(&target_op),
        Some(&valid_sig),
        "The valid (old) signature should be retained"
    );
}

/// Old signature is garbage, new signature is valid. The map should be updated
/// to contain the new valid signature.
#[test]
fn resolve_duplicate_replaces_with_valid() {
    // Arrange
    let material = create_test_key_material();
    let garbage = create_garbage_key_material();
    let share_pubkeys = build_share_pubkey_map(&material);

    let target_op = material.shares[0].0;
    let valid_sig = material.shares[0].1.sign(material.signing_root);
    let garbage_sig = garbage.shares[0].1.sign(material.signing_root);

    // Start with the garbage sig in the map (simulates old invalid share)
    let mut shares = HashMap::new();
    shares.insert(target_op, garbage_sig);

    // Act
    resolve_duplicate_signature(
        &mut shares,
        target_op,
        &valid_sig,
        material.signing_root,
        &share_pubkeys,
    );

    // Assert
    assert!(
        shares.contains_key(&target_op),
        "Map should contain the operator after replacement"
    );
    assert_eq!(
        shares.get(&target_op),
        Some(&valid_sig),
        "The new valid signature should replace the old garbage one"
    );
}

/// Both old and new signatures are garbage. The operator should be removed
/// from the map entirely.
#[test]
fn resolve_duplicate_removes_both() {
    // Arrange
    let material = create_test_key_material();
    let garbage = create_garbage_key_material();
    let share_pubkeys = build_share_pubkey_map(&material);

    let target_op = material.shares[0].0;

    // Create two different garbage signatures using two different garbage key sets
    let garbage_old = garbage.shares[0].1.sign(material.signing_root);

    // Use a different signing root to produce a distinct garbage signature
    let different_root = Hash256::from([0xCD; 32]);
    let garbage_new = garbage.shares[0].1.sign(different_root);

    let mut shares = HashMap::new();
    shares.insert(target_op, garbage_old);

    // Act
    resolve_duplicate_signature(
        &mut shares,
        target_op,
        &garbage_new,
        material.signing_root,
        &share_pubkeys,
    );

    // Assert
    assert!(
        !shares.contains_key(&target_op),
        "Map should not contain the operator when both signatures are invalid"
    );
}

// ==================== Integration test helpers ====================

const TEST_NETWORK: &str = "test";

/// Creates an in-memory `NetworkDatabase` seeded with a validator and operator shares
/// whose `share_pubkey` values are real BLS public keys derived from the test key material.
/// Returns the database for use with the `signature_collector()` async task.
fn create_seeded_database(material: &TestKeyMaterial) -> Arc<NetworkDatabase> {
    let rsa_pubkey = generators::pubkey::random_rsa();
    let db = NetworkDatabase::new_in_memory(&rsa_pubkey, TEST_NETWORK)
        .expect("Failed to create in-memory database");

    let mut conn = db.connection().expect("Failed to get connection");
    let tx = conn.transaction().expect("Failed to begin transaction");

    // Create operators
    let operators: Vec<_> = material
        .shares
        .iter()
        .map(|(op_id, _)| generators::operator::with_id(**op_id))
        .collect();
    for op in &operators {
        db.insert_operator(op, &tx)
            .expect("Failed to insert operator");
    }

    // Create cluster with these operators
    let cluster = generators::cluster::with_operators(&operators);

    // Build shares with real BLS share pubkeys from key material
    let shares: Vec<Share> = material
        .shares
        .iter()
        .map(|(op_id, sk)| Share {
            validator_pubkey: material.master_pubkey_bytes,
            operator_id: *op_id,
            cluster_id: cluster.cluster_id,
            share_pubkey: PublicKeyBytes::from(sk.public_key()),
            encrypted_private_key: [0u8; ENCRYPTED_KEY_LENGTH],
        })
        .collect();

    // Create validator metadata with the real master pubkey
    let validator = ValidatorMetadata {
        public_key: material.master_pubkey_bytes,
        cluster_id: cluster.cluster_id,
        index: Some(ValidatorIndex(1)),
        graffiti: Graffiti::default(),
    };

    db.insert_validator(cluster, &validator, shares, &tx)
        .expect("Failed to insert validator");

    tx.commit().expect("Failed to commit transaction");

    Arc::new(db)
}

/// Sends a `RegisterNotifier` message to the collector channel.
/// Returns the `oneshot::Receiver` that will receive the reconstructed signature.
fn send_register_notifier(
    tx: &mpsc::UnboundedSender<CollectorMessage>,
    threshold: u64,
    validator_pubkey: PublicKeyBytes,
) -> oneshot::Receiver<Arc<Signature>> {
    let (result_tx, result_rx) = oneshot::channel();
    tx.send(CollectorMessage {
        kind: CollectorMessageKind::RegisterNotifier {
            notify: result_tx,
            threshold,
            validator_pubkey,
        },
        _drop_on_finish: None,
    })
    .expect("Failed to send RegisterNotifier");
    result_rx
}

/// Sends a `PartialSignature` message to the collector channel.
fn send_partial_sig(
    tx: &mpsc::UnboundedSender<CollectorMessage>,
    operator_id: OperatorId,
    signature: Signature,
) {
    tx.send(CollectorMessage {
        kind: CollectorMessageKind::PartialSignature {
            operator_id,
            signature: Box::new(signature),
        },
        _drop_on_finish: None,
    })
    .expect("Failed to send PartialSignature");
}

// ==================== Integration tests ====================

/// End-to-end happy path: 3 valid shares reach threshold, collector reconstructs
/// a valid signature and notifies the waiter.
#[tokio::test]
async fn integration_happy_path_all_valid() {
    // Arrange
    let material = create_test_key_material();
    let db = create_seeded_database(&material);

    let (tx, rx) = mpsc::unbounded_channel();
    let signing_root = material.signing_root;

    // Spawn the collector task
    let handle = tokio::spawn(signature_collector(rx, signing_root, db));

    // Register a notifier expecting threshold 3
    let result_rx = send_register_notifier(&tx, THRESHOLD, material.master_pubkey_bytes);

    // Send 3 valid partial signatures
    for (op_id, sk) in &material.shares[..3] {
        send_partial_sig(&tx, *op_id, sk.sign(signing_root));
    }

    // Assert: notifier receives a valid reconstructed signature
    let reconstructed = result_rx
        .await
        .expect("Should receive reconstructed signature");

    // Verify the reconstructed signature against the master pubkey
    assert!(
        verify_reconstructed_signature(&reconstructed, &material.master_pubkey_bytes, signing_root),
        "Reconstructed signature should verify against master pubkey"
    );

    // Clean up: drop sender so the collector task exits
    drop(tx);
    handle.await.expect("Collector task should complete");
}

///  2 valid + 1 garbage share reach threshold,
/// verification fails, garbage operator's share is removed, then a 4th valid
/// share brings us back to threshold and the collector succeeds.
#[tokio::test]
async fn integration_fallback_removes_bad_and_succeeds() {
    // Arrange
    let material = create_test_key_material();
    let garbage = create_garbage_key_material();
    let db = create_seeded_database(&material);

    let (tx, rx) = mpsc::unbounded_channel();
    let signing_root = material.signing_root;

    let handle = tokio::spawn(signature_collector(rx, signing_root, db));
    let result_rx = send_register_notifier(&tx, THRESHOLD, material.master_pubkey_bytes);

    // Send 2 valid partial signatures
    for (op_id, sk) in &material.shares[..2] {
        send_partial_sig(&tx, *op_id, sk.sign(signing_root));
    }

    // Send 1 garbage partial signature (operator 3 with wrong key)
    let bad_op = material.shares[2].0;
    let garbage_sig = garbage.shares[2].1.sign(signing_root);
    send_partial_sig(&tx, bad_op, garbage_sig);

    // At this point threshold is reached but verification fails.
    // The collector removes operator 3's invalid share, dropping below threshold.
    // Now send the 4th valid share to bring us back to threshold.
    let (op4, sk4) = &material.shares[3];
    send_partial_sig(&tx, *op4, sk4.sign(signing_root));

    // Assert: notifier receives a valid reconstructed signature
    let reconstructed = result_rx
        .await
        .expect("Should receive reconstructed signature");
    assert!(
        verify_reconstructed_signature(&reconstructed, &material.master_pubkey_bytes, signing_root),
        "Reconstructed signature should verify after fallback + recovery"
    );

    drop(tx);
    handle.await.expect("Collector task should complete");
}

/// Duplicate resolution via DB lookup: a valid sig is already held for an operator,
/// then a garbage duplicate arrives. The collector should keep the valid sig.
#[tokio::test]
async fn integration_duplicate_resolution_keeps_valid() {
    // Arrange
    let material = create_test_key_material();
    let garbage = create_garbage_key_material();
    let db = create_seeded_database(&material);

    let (tx, rx) = mpsc::unbounded_channel();
    let signing_root = material.signing_root;

    let handle = tokio::spawn(signature_collector(rx, signing_root, db));
    let result_rx = send_register_notifier(&tx, THRESHOLD, material.master_pubkey_bytes);

    // Send valid sig for operator 1
    let (op1, sk1) = &material.shares[0];
    send_partial_sig(&tx, *op1, sk1.sign(signing_root));

    // Send valid sig for operator 2
    let (op2, sk2) = &material.shares[1];
    send_partial_sig(&tx, *op2, sk2.sign(signing_root));

    // Send a garbage duplicate for operator 1 — triggers resolve_duplicate_signature
    let garbage_dup = garbage.shares[0].1.sign(signing_root);
    send_partial_sig(&tx, *op1, garbage_dup);

    // Send valid sig for operator 3 to reach threshold
    let (op3, sk3) = &material.shares[2];
    send_partial_sig(&tx, *op3, sk3.sign(signing_root));

    // Assert: notifier receives valid signature (op1's valid sig was retained)
    let reconstructed = result_rx
        .await
        .expect("Should receive reconstructed signature");
    assert!(
        verify_reconstructed_signature(&reconstructed, &material.master_pubkey_bytes, signing_root),
        "Reconstructed signature should verify because valid duplicate was kept"
    );

    drop(tx);
    handle.await.expect("Collector task should complete");
}

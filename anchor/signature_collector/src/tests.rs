use std::collections::HashMap;

use bls::{PublicKeyBytes, SecretKey, Signature};
use bls_lagrange::{KeyId, split_with_rng};
use rand::{prelude::*, rngs::StdRng};
use types::Hash256;

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

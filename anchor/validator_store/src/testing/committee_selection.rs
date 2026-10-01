//! Boole+ `AggregatorCommittee` selection batches started by publishing `VotingAssignments`.
//!
//! Most tests observe the harness's mock collector on paused time: which validators and roots a
//! publication plans, how often each committee is signed and sent, retries and the selection
//! deadline, and what Lighthouse's selection callbacks do afterwards. The last two run the real
//! collector and processor: one sends a batch without any Lighthouse callback, the other resolves
//! late callbacks with real keys.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, mpsc as std_mpsc},
    time::Duration,
};

use bls::{PublicKeyBytes, SecretKey};
use database::{NetworkDatabase, OwnOperatorId, PendingStateUpdates, UniqueIndex};
use fork::{Fork, ForkSchedule};
use message_sender::{Error as SendError, MessageSender};
use openssl::{
    pkey::Private,
    rsa::{Padding, Rsa},
};
use signature_collector::{CommitteeSelectionBatch, SignatureCollectorManager, SignatureRequester};
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::{
    ClusterId, CommitteeId, ENCRYPTED_KEY_LENGTH, OperatorId, ValidatorIndex, ValidatorMetadata,
    VariableList,
    consensus::UnsignedSSVMessage,
    domain_type::DomainType,
    message::SignedSSVMessage,
    msgid::{DutyExecutor, Role},
    partial_sig::{PartialSignatureKind, PartialSignatureMessage, PartialSignatureMessages},
    typenum::Unsigned,
};
use ssz::{Decode, Encode};
use tokio::{sync::mpsc, time::Instant};
use types::{
    Address, Domain, Epoch, EthSpec, Graffiti, Hash256, MainnetEthSpec, SelectionProof, SignedRoot,
    Slot, SyncSelectionProof, SyncSubnetId,
};
use validator_store::ValidatorStore;

use super::common::*;
use crate::{Error, SpecificError, VotingAssignments};

const OUR_OPERATOR_ID: OperatorId = OperatorId(1);
/// Pause between send or signing attempts: `RETRY_INTERVAL` in `crate::committee_selection`.
const RETRY_INTERVAL: Duration = Duration::from_millis(250);
/// The selection deadline, two thirds into the slot.
const SELECTION_DEADLINE_IN_SLOT: Duration = Duration::from_secs(SLOT_DURATION_SECS * 2 / 3);
/// The harness clock's offset into `TEST_SLOT`. Publishing at this offset into any slot puts its
/// selection deadline `SELECTION_DEADLINE_AFTER_PUBLICATION` ahead.
const PUBLICATION_IN_SLOT: Duration = Duration::from_secs(CLOCK_OFFSET_INTO_TEST_SLOT_SECS);
/// How long the real-collector test lets late callbacks run on this operator's share alone.
const OWN_SHARE_ONLY: Duration = Duration::from_millis(200);
/// How long a second envelope for the same committee and slot is given to appear.
const DUPLICATE_GRACE: Duration = Duration::from_millis(500);

// ==================== Helpers ====================

/// One selection batch entry as `(validator index, validator pubkey, signing root)`.
type SelectionEntry = (usize, PublicKeyBytes, Hash256);

fn one_committee_harness(validators: usize) -> ValidatorStoreTestHarness {
    ValidatorStoreTestHarness::new(
        vec![create_primary_committee_setup(validators)],
        OUR_OPERATOR_ID,
    )
}

/// `VotingAssignments` at `slot` giving each of `attesters` an attester duty and nothing else.
fn attester_assignments<'a>(
    slot: Slot,
    attesters: impl IntoIterator<Item = &'a ValidatorMetadata>,
) -> VotingAssignments {
    let mut assignments = VotingAssignments {
        slot,
        attesting_validators: Vec::new(),
        attesting_committees: HashMap::new(),
        sync_validators_by_subnet: HashMap::new(),
    };
    for (committee_index, validator) in attesters.into_iter().enumerate() {
        assignments
            .attesting_validators
            .push(validator.index.expect("test attester has an index"));
        assignments
            .attesting_committees
            .insert(validator.public_key, committee_index as u64);
    }
    assignments
}

/// Gives `validator` a sync duty with the given `(subnet, positions)` pairs.
fn add_sync_duty(
    assignments: &mut VotingAssignments,
    validator: &ValidatorMetadata,
    subnet_positions: impl IntoIterator<Item = (u64, usize)>,
) {
    assignments.sync_validators_by_subnet.insert(
        validator.index.expect("test sync member has an index"),
        subnet_positions
            .into_iter()
            .map(|(subnet, positions)| (SyncSubnetId::new(subnet), positions))
            .collect(),
    );
}

fn attestation_entry(
    harness: &ValidatorStoreTestHarness,
    validator: &ValidatorMetadata,
    slot: Slot,
) -> SelectionEntry {
    let domain = harness.validator_store.get_domain(
        slot.epoch(MainnetEthSpec::slots_per_epoch()),
        Domain::SelectionProof,
    );
    (
        validator.index.expect("test validator has an index").0,
        validator.public_key,
        slot.signing_root(domain),
    )
}

fn sync_entry(
    harness: &ValidatorStoreTestHarness,
    validator: &ValidatorMetadata,
    slot: Slot,
    subnet: u64,
) -> SelectionEntry {
    (
        validator.index.expect("test validator has an index").0,
        validator.public_key,
        harness
            .validator_store
            .compute_sync_selection_root(slot, subnet),
    )
}

/// Batch order: ascending validator index, then root.
fn sorted(mut entries: Vec<SelectionEntry>) -> Vec<SelectionEntry> {
    entries.sort_unstable_by_key(|(index, _, root)| (*index, *root));
    entries
}

fn batch_entries(batch: &CommitteeSelectionBatch) -> Vec<SelectionEntry> {
    batch
        .signing_data()
        .iter()
        .map(|entry| (entry.index.0, entry.validator_pubkey, entry.root))
        .collect()
}

/// The one batch signed so far.
fn only_signed_batch(harness: &ValidatorStoreTestHarness) -> Arc<CommitteeSelectionBatch> {
    let batches = harness.committee_selection.signed_batches();
    assert_eq!(
        batches.len(),
        1,
        "exactly one committee selection batch is signed"
    );
    Arc::clone(&batches[0])
}

fn decode_envelope(message: &UnsignedSSVMessage) -> PartialSignatureMessages {
    PartialSignatureMessages::from_ssz_bytes(message.ssv_message.data())
        .expect("the committee envelope decodes")
}

/// The `(validator index, signing root)` pairs a committee envelope carries, in wire order.
fn envelope_pairs(message: &UnsignedSSVMessage) -> Vec<(usize, Hash256)> {
    decode_envelope(message)
        .messages
        .iter()
        .map(|message| (message.validator_index.0, message.signing_root))
        .collect()
}

fn pairs(entries: &[SelectionEntry]) -> Vec<(usize, Hash256)> {
    entries
        .iter()
        .map(|(index, _, root)| (*index, *root))
        .collect()
}

fn committee_of(harness: &ValidatorStoreTestHarness, validator: &ValidatorMetadata) -> CommitteeId {
    harness
        .validator_store
        .get_validator_and_cluster(validator.public_key)
        .expect("registered validator")
        .1
        .committee_id()
}

/// Moves the harness clock to `into_slot` past the start of `slot`.
fn set_clock(harness: &ValidatorStoreTestHarness, slot: Slot, into_slot: Duration) {
    let start = harness
        .slot_clock
        .start_of(slot)
        .expect("slot start is known");
    harness.slot_clock.set_current_time(start + into_slot);
}

/// Publishes `assignments` as the slot pipeline does, and runs past their selection deadline.
async fn publish_and_settle(harness: &ValidatorStoreTestHarness, assignments: VotingAssignments) {
    harness
        .validator_store
        .update_voting_assignments(assignments);
    run_past_selection_deadline().await;
}

/// Waits until `condition` holds, well within any selection deadline.
async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the condition should hold promptly");
}

/// Asserts that the only network send was the committee's selection batch, and that every
/// Lighthouse callback made so far only signed locally, which never sends.
fn assert_only_the_batch_was_sent(harness: &ValidatorStoreTestHarness) {
    let sends = harness.committee_selection.sends();
    assert_eq!(
        sends.len(),
        1,
        "only the committee's selection batch is sent"
    );
    assert!(sends[0].admitted);
    assert!(
        harness
            .captured_calls
            .lock()
            .iter()
            .all(|call| matches!(call.requester, SignatureRequester::LocalOnly)),
        "a selection callback only signs locally"
    );
}

/// Liquidates `cluster_id` through the database call the `ClusterLiquidated` event processor
/// makes, which only flips the flag and keeps the cluster's validators and shares.
fn liquidate_cluster(database: &NetworkDatabase, cluster_id: ClusterId) {
    let mut conn = database.connection().expect("connection should succeed");
    let tx = conn.transaction().expect("transaction should start");
    let mut pending = PendingStateUpdates::default();
    database
        .update_status_tx(cluster_id, true, &tx, &mut pending)
        .expect("liquidation should succeed");
    tx.commit().expect("commit should succeed");
    database.publish_pending_state_updates(pending);
}

fn generate_operator_key() -> Rsa<Private> {
    Rsa::generate(2048).expect("RSA operator key generation")
}

/// Encrypts `share` to `operator_key` as a share registration carries it: the hex-encoded secret
/// key, PKCS#1 v1.5 padded.
fn encrypt_share(operator_key: &Rsa<Private>, share: &SecretKey) -> [u8; ENCRYPTED_KEY_LENGTH] {
    let plaintext = format!("0x{}", hex::encode(share.serialize().as_bytes()));
    let mut ciphertext = [0; ENCRYPTED_KEY_LENGTH];
    let length = operator_key
        .public_encrypt(plaintext.as_bytes(), &mut ciphertext, Padding::PKCS1)
        .expect("share encryption");
    assert_eq!(length, ENCRYPTED_KEY_LENGTH);
    ciphertext
}

/// Gives the store `operator_key`, so it decrypts its shares instead of running as an impostor.
fn install_operator_key(harness: &mut ValidatorStoreTestHarness, operator_key: Rsa<Private>) {
    Arc::get_mut(&mut harness.validator_store)
        .expect("the store is not shared before the test publishes anything")
        .private_key = Some(operator_key);
}

/// Locks the store's decrypted key cache from another thread until the returned sender is
/// dropped. With an operator key installed, loading any share takes that lock, so key loading
/// cannot finish before then.
fn hold_decrypted_keys(harness: &ValidatorStoreTestHarness) -> std_mpsc::Sender<()> {
    let store = Arc::clone(&harness.validator_store);
    let (held_tx, held) = std_mpsc::channel();
    let (release, released) = std_mpsc::channel::<()>();
    std::thread::spawn(move || {
        let _cache = store.decrypted_keys.lock();
        held_tx.send(()).expect("the test waits for the lock");
        // Only ever ends by the sender being dropped.
        let _ = released.recv();
    });
    held.recv().expect("the cache is locked");
    release
}

/// Records every message the collector offers for sending.
struct AdmissionRecorder(mpsc::UnboundedSender<UnsignedSSVMessage>);

impl MessageSender for AdmissionRecorder {
    fn sign_and_send(
        &self,
        message: UnsignedSSVMessage,
        _: CommitteeId,
        _: Option<Box<dyn FnOnce(&SignedSSVMessage) + Send + 'static>>,
    ) -> Result<(), SendError> {
        self.0
            .send(message)
            .map_err(|_| SendError::NetworkQueueClosed)
    }

    fn send(&self, _: SignedSSVMessage, _: CommitteeId) -> Result<(), SendError> {
        Ok(())
    }
}

/// Replaces the harness's mock collector with a real `SignatureCollectorManager` on a real
/// processor, acting as `operator_id`. Every message it offers for sending arrives on the returned
/// receiver.
fn install_recording_collector(
    harness: &mut ValidatorStoreTestHarness,
    operator_id: OperatorId,
) -> (
    Arc<SignatureCollectorManager<ManualSlotClock>>,
    mpsc::UnboundedReceiver<UnsignedSSVMessage>,
) {
    let (sender, admitted) = mpsc::unbounded_channel();
    let store = Arc::get_mut(&mut harness.validator_store)
        .expect("the store is not shared before the test publishes anything");
    let processor = processor::spawn(processor::Config::default(), store.task_executor.clone());
    let collector = SignatureCollectorManager::new(
        processor,
        OwnOperatorId::Known(operator_id),
        Arc::clone(&store.database),
        Arc::clone(&store.fork_schedule),
        MainnetEthSpec::slots_per_epoch(),
        Arc::new(AdmissionRecorder(sender)),
        harness.slot_clock.clone(),
    )
    .expect("the real collector starts");
    store.signature_collector = Box::new(Arc::clone(&collector));
    (collector, admitted)
}

// ==================== Which validators and roots are batched ====================

/// Boole activates at epoch 1. Assignments for the last pre-Boole slot start nothing; the same
/// assignments for the first Boole slot sign and send the batch.
#[tokio::test(start_paused = true)]
async fn selection_batches_start_at_the_first_boole_slot() {
    // Arrange
    let mut harness = one_committee_harness(1);
    let boole_epoch = Epoch::new(1);
    let fork_schedule = ForkSchedule::from_fork_configs(
        BTreeMap::from([
            (Fork::Alan, (Epoch::new(0), DomainType([0, 0, 0, 1]))),
            (Fork::Boole, (boole_epoch, DomainType([0, 0, 0, 2]))),
        ]),
        "test",
    )
    .expect("the test fork schedule is valid");
    Arc::get_mut(&mut harness.validator_store)
        .expect("the store is not shared before the test publishes anything")
        .fork_schedule = Arc::new(fork_schedule);
    let validator = harness.validator_metadata(0, 0);
    let first_boole_slot = boole_epoch.start_slot(MainnetEthSpec::slots_per_epoch());
    let last_pre_boole_slot = first_boole_slot - 1;

    // Act and assert: pre-Boole.
    set_clock(&harness, last_pre_boole_slot, PUBLICATION_IN_SLOT);
    publish_and_settle(
        &harness,
        attester_assignments(last_pre_boole_slot, [&validator]),
    )
    .await;
    assert!(harness.committee_selection.signed_batches().is_empty());
    assert!(harness.committee_selection.sends().is_empty());

    // Act and assert: Boole.
    set_clock(&harness, first_boole_slot, PUBLICATION_IN_SLOT);
    publish_and_settle(
        &harness,
        attester_assignments(first_boole_slot, [&validator]),
    )
    .await;
    assert_eq!(
        batch_entries(&only_signed_batch(&harness)),
        vec![attestation_entry(&harness, &validator, first_boole_slot)]
    );
    assert_eq!(harness.committee_selection.sends().len(), 1);
}

/// A newly registered validator can hold an attester duty before its index is known. It is left
/// out rather than withholding the other members' proofs, as is an index-less validator without
/// any duty, and its own callback fails before signing.
#[tokio::test(start_paused = true)]
async fn attester_without_index_is_left_out_of_the_batch() {
    // Arrange: validator 1 attests without an index; validator 2 has neither index nor duty.
    let mut committee = create_primary_committee_setup(3);
    committee.validators[1].index = None;
    committee.validators[2].index = None;
    let harness = ValidatorStoreTestHarness::new(vec![committee], OUR_OPERATOR_ID);
    let indexed = harness.validator_metadata(0, 0);
    let unindexed_attester = harness.validator_metadata(0, 1);
    let slot = Slot::new(TEST_SLOT);
    let mut assignments = attester_assignments(slot, [&indexed]);
    assignments
        .attesting_committees
        .insert(unindexed_attester.public_key, 1);

    // Act
    publish_and_settle(&harness, assignments).await;
    let callback = harness
        .validator_store
        .produce_selection_proof(unindexed_attester.public_key, slot)
        .await;
    run_past_selection_deadline().await;

    // Assert
    assert_eq!(
        batch_entries(&only_signed_batch(&harness)),
        vec![attestation_entry(&harness, &indexed, slot)]
    );
    assert!(matches!(
        callback,
        Err(Error::SpecificError(SpecificError::MissingIndex))
    ));
    assert_only_the_batch_was_sent(&harness);
}

/// Lighthouse keeps a liquidated cluster's sync duties for the rest of the sync period, and
/// liquidation keeps the validator's metadata, so a liquidated sync member stays in
/// `VotingAssignments` every slot. It is left out, not allowed to withhold its committee's batch.
#[tokio::test(start_paused = true)]
async fn liquidated_sync_member_is_left_out_of_the_batch() {
    // Arrange: two clusters over the primary operators, so one committee. The second is
    // liquidated after registration while its validator keeps a sync duty.
    let active_setup = create_committee_setup(
        &PRIMARY_COMMITTEE_OPERATOR_IDS,
        1,
        PRIMARY_COMMITTEE_STARTING_VALIDATOR_INDEX,
    );
    let mut liquidated_setup = create_committee_setup(
        &PRIMARY_COMMITTEE_OPERATOR_IDS,
        1,
        PRIMARY_COMMITTEE_STARTING_VALIDATOR_INDEX + 1,
    );
    // A cluster is identified by owner and operators, so a second cluster over the same
    // operators has another owner.
    liquidated_setup.cluster.owner = Address::repeat_byte(1);
    let harness =
        ValidatorStoreTestHarness::new(vec![active_setup, liquidated_setup], OUR_OPERATOR_ID);
    let active = harness.validator_metadata(0, 0);
    let liquidated = harness.validator_metadata(1, 0);
    assert_eq!(
        committee_of(&harness, &active),
        committee_of(&harness, &liquidated)
    );
    liquidate_cluster(&harness.validator_store.database, liquidated.cluster_id);
    let slot = Slot::new(TEST_SLOT);
    let mut assignments = attester_assignments(slot, [&active]);
    add_sync_duty(&mut assignments, &liquidated, [(0, 1)]);

    // Act
    publish_and_settle(&harness, assignments).await;
    let callback = harness
        .validator_store
        .produce_sync_selection_proof(&liquidated.public_key, slot, SyncSubnetId::new(0))
        .await;
    run_past_selection_deadline().await;

    // Assert
    assert_eq!(
        batch_entries(&only_signed_batch(&harness)),
        vec![attestation_entry(&harness, &active, slot)]
    );
    assert!(matches!(
        callback,
        Err(Error::SpecificError(SpecificError::ClusterLiquidated))
    ));
    assert_only_the_batch_was_sent(&harness);
}

/// `attesting_validators` and `attesting_committees` disagreeing about a validator leaves only
/// that validator out, in either direction and without any sync duty.
#[tokio::test(start_paused = true)]
async fn inconsistent_attester_assignments_are_left_out_of_the_batch() {
    // Arrange: validator 0 is a consistent attester. Validator 1's index is listed as attesting
    // without its pubkey, and validator 2's pubkey without its index.
    let harness = one_committee_harness(3);
    let consistent = harness.validator_metadata(0, 0);
    let index_only = harness.validator_metadata(0, 1);
    let pubkey_only = harness.validator_metadata(0, 2);
    let slot = Slot::new(TEST_SLOT);
    let mut assignments = attester_assignments(slot, [&consistent]);
    assignments
        .attesting_validators
        .push(index_only.index.expect("test validator has an index"));
    assignments
        .attesting_committees
        .insert(pubkey_only.public_key, 2);

    // Act
    publish_and_settle(&harness, assignments).await;
    let index_only_callback = harness
        .validator_store
        .produce_selection_proof(index_only.public_key, slot)
        .await;
    let pubkey_only_callback = harness
        .validator_store
        .produce_selection_proof(pubkey_only.public_key, slot)
        .await;
    run_past_selection_deadline().await;

    // Assert: the pubkey-only callback passes the callback's own check and signs locally.
    assert_eq!(
        batch_entries(&only_signed_batch(&harness)),
        vec![attestation_entry(&harness, &consistent, slot)]
    );
    assert!(matches!(
        index_only_callback,
        Err(Error::SpecificError(
            SpecificError::ValidatorNotAttesting { .. }
        ))
    ));
    pubkey_only_callback.expect("the callback only signs locally");
    assert_only_the_batch_was_sent(&harness);
}

/// An invalid sync subnet map leaves out the whole validator, including any valid subnet beside
/// the invalid one, while other members keep their attestation and sync entries.
#[tokio::test(start_paused = true)]
async fn invalid_sync_subnet_assignments_are_left_out_of_the_batch() {
    // Arrange: validator 0 attests and has a valid sync duty. Validators 1 to 3 have an empty
    // subnet map, a zero position count beside a valid subnet, and an out-of-range subnet.
    let harness = one_committee_harness(4);
    let valid = harness.validator_metadata(0, 0);
    let empty_map = harness.validator_metadata(0, 1);
    let zero_count = harness.validator_metadata(0, 2);
    let out_of_range = harness.validator_metadata(0, 3);
    let subnet_count = <MainnetEthSpec as EthSpec>::SyncCommitteeSubnetCount::to_u64();
    let slot = Slot::new(TEST_SLOT);
    let mut assignments = attester_assignments(slot, [&valid]);
    add_sync_duty(&mut assignments, &valid, [(1, 1)]);
    add_sync_duty(&mut assignments, &empty_map, []);
    add_sync_duty(&mut assignments, &zero_count, [(0, 1), (2, 0)]);
    add_sync_duty(&mut assignments, &out_of_range, [(subnet_count, 1)]);

    // Act
    publish_and_settle(&harness, assignments).await;
    for (validator, subnet) in [
        (&empty_map, 0),
        (&zero_count, 0),
        (&out_of_range, subnet_count),
    ] {
        harness
            .validator_store
            .produce_sync_selection_proof(&validator.public_key, slot, SyncSubnetId::new(subnet))
            .await
            .expect("the callback only signs locally");
    }
    run_past_selection_deadline().await;

    // Assert
    assert_eq!(
        batch_entries(&only_signed_batch(&harness)),
        sorted(vec![
            attestation_entry(&harness, &valid, slot),
            sync_entry(&harness, &valid, slot, 1),
        ])
    );
    assert_only_the_batch_was_sent(&harness);
}

/// Committee metadata can list a validator this operator holds no share for. Only this operator's
/// shares are planned, so it never enters the batch, and its callback fails before signing.
#[tokio::test(start_paused = true)]
async fn validator_without_local_share_is_left_out_of_the_batch() {
    // Arrange: a second validator in the same cluster, registered with no shares, holds an
    // attester duty.
    const UNSHARED_VALIDATOR_INDEX: usize = 99;
    let mut committee = create_primary_committee_setup(1);
    committee.validators.push(ValidatorMetadata {
        public_key: PublicKeyBytes::deserialize(&[UNSHARED_VALIDATOR_INDEX as u8; 48])
            .expect("valid length"),
        cluster_id: committee.cluster.cluster_id,
        index: Some(ValidatorIndex(UNSHARED_VALIDATOR_INDEX)),
        graffiti: Graffiti::default(),
    });
    let harness = ValidatorStoreTestHarness::new(vec![committee], OUR_OPERATOR_ID);
    let shared = harness.validator_metadata(0, 0);
    let unshared = harness.validator_metadata(0, 1);
    {
        let state = harness.validator_store.database.state();
        assert!(state.metadata().get_by(&unshared.public_key).is_some());
        assert!(state.shares().get_by(&unshared.public_key).is_none());
    }
    let slot = Slot::new(TEST_SLOT);

    // Act
    publish_and_settle(&harness, attester_assignments(slot, [&shared, &unshared])).await;
    let callback = harness
        .validator_store
        .produce_selection_proof(unshared.public_key, slot)
        .await;
    run_past_selection_deadline().await;

    // Assert
    assert_eq!(
        batch_entries(&only_signed_batch(&harness)),
        vec![attestation_entry(&harness, &shared, slot)]
    );
    assert!(matches!(callback, Err(Error::UnknownPubkey(pubkey)) if pubkey == unshared.public_key));
    assert_only_the_batch_was_sent(&harness);
}

/// A validator whose share this operator cannot decrypt is left out when the signing keys are
/// loaded, while the others keep their decrypted shares. Its callback fails before signing.
#[tokio::test(start_paused = true)]
async fn validator_with_undecryptable_share_is_left_out_of_the_batch() {
    // Arrange: validator 0's share is encrypted to the operator key; validator 1 keeps the
    // harness's placeholder, which is not.
    let operator_key = generate_operator_key();
    let validator_key = SecretKey::random();
    let mut committee = create_primary_committee_setup(2);
    committee.set_validator_key(
        0,
        validator_key.public_key().compress(),
        encrypt_share(&operator_key, &validator_key),
    );
    let mut harness = ValidatorStoreTestHarness::new(vec![committee], OUR_OPERATOR_ID);
    install_operator_key(&mut harness, operator_key);
    let decryptable = harness.validator_metadata(0, 0);
    let undecryptable = harness.validator_metadata(0, 1);
    let slot = Slot::new(TEST_SLOT);

    // Act
    publish_and_settle(
        &harness,
        attester_assignments(slot, [&decryptable, &undecryptable]),
    )
    .await;
    let callback = harness
        .validator_store
        .produce_selection_proof(undecryptable.public_key, slot)
        .await;
    run_past_selection_deadline().await;

    // Assert
    let batch = only_signed_batch(&harness);
    assert_eq!(
        batch_entries(&batch),
        vec![attestation_entry(&harness, &decryptable, slot)]
    );
    assert_eq!(
        batch.signing_data()[0]
            .share
            .as_ref()
            .map(|share| share.public_key().compress()),
        Some(validator_key.public_key().compress()),
        "the batch carries the decrypted share"
    );
    assert!(matches!(
        callback,
        Err(Error::SpecificError(
            SpecificError::KeyShareDecryptionFailed
        ))
    ));
    assert_only_the_batch_was_sent(&harness);
}

/// Control for the skip tests: ten validators on all four sync subnets, one of them also
/// attesting, batch exactly one attestation root and forty distinct subnet roots, whatever the
/// position counts.
#[tokio::test(start_paused = true)]
async fn ten_validators_on_four_subnets_batch_forty_one_entries() {
    // Arrange
    const VALIDATORS: usize = 10;
    const SUBNETS: u64 = 4;
    const POSITIONS_PER_SUBNET: usize = 2;
    const EXPECTED_ENTRIES: usize = 1 + VALIDATORS * SUBNETS as usize;
    let harness = one_committee_harness(VALIDATORS);
    let attester = harness.validator_metadata(0, 0);
    let slot = Slot::new(TEST_SLOT);
    let mut assignments = attester_assignments(slot, [&attester]);
    let mut expected = vec![attestation_entry(&harness, &attester, slot)];
    for position in 0..VALIDATORS {
        let validator = harness.validator_metadata(0, position);
        add_sync_duty(
            &mut assignments,
            &validator,
            (0..SUBNETS).map(|subnet| (subnet, POSITIONS_PER_SUBNET)),
        );
        expected.extend((0..SUBNETS).map(|subnet| sync_entry(&harness, &validator, slot, subnet)));
    }

    // Act
    publish_and_settle(&harness, assignments).await;

    // Assert
    let entries = batch_entries(&only_signed_batch(&harness));
    assert_eq!(entries.len(), EXPECTED_ENTRIES);
    assert_eq!(entries, sorted(expected));
    let sends = harness.committee_selection.sends();
    assert_eq!(sends.len(), 1);
    assert_eq!(envelope_pairs(&sends[0].message), pairs(&entries));
}

// ==================== One execution per committee and slot ====================

/// Publishing the same slot again, first while the committee's signing is in flight and again
/// after it finished, sends that committee's first worklist exactly once. A committee that only
/// has work in the second publication starts then, once.
#[tokio::test(start_paused = true)]
async fn repeated_publication_sends_first_worklist_once_and_starts_new_committees() {
    // Arrange: committee A (primary) and committee B (secondary).
    let harness = ValidatorStoreTestHarness::new(
        vec![
            create_primary_committee_setup(2),
            create_secondary_committee_setup(1),
        ],
        OUR_OPERATOR_ID,
    );
    let a_first = harness.validator_metadata(0, 0);
    let a_second = harness.validator_metadata(0, 1);
    let b_only = harness.validator_metadata(1, 0);
    let committee_a = committee_of(&harness, &a_first);
    let committee_b = committee_of(&harness, &b_only);
    assert_ne!(committee_a, committee_b);
    let slot = Slot::new(TEST_SLOT);
    let selection = &harness.committee_selection;
    selection.hold_signing();

    // Act: the first publication gives A one attester. While A's signing is held, the second
    // gives A another attester and B its first work. A third, after both executions finished,
    // repeats the first.
    harness
        .validator_store
        .update_voting_assignments(attester_assignments(slot, [&a_first]));
    wait_until(|| selection.signed_batches().len() == 1).await;
    assert!(selection.sends().is_empty(), "A's signing is still held");
    harness
        .validator_store
        .update_voting_assignments(attester_assignments(slot, [&a_first, &a_second, &b_only]));
    wait_until(|| selection.signed_batches().len() >= 2).await;
    selection.release_signing();
    run_past_selection_deadline().await;
    publish_and_settle(&harness, attester_assignments(slot, [&a_first])).await;

    // Assert
    let sends = selection.sends();
    let sends_to = |committee| {
        sends
            .iter()
            .filter(|send| send.committee_id == committee)
            .collect::<Vec<_>>()
    };
    let (a_sends, b_sends) = (sends_to(committee_a), sends_to(committee_b));
    assert_eq!(a_sends.len(), 1, "committee A is sent exactly once");
    assert_eq!(b_sends.len(), 1, "committee B is sent exactly once");
    assert_eq!(sends.len(), 2);
    assert_eq!(
        envelope_pairs(&a_sends[0].message),
        pairs(&[attestation_entry(&harness, &a_first, slot)]),
        "A's envelope carries the first publication's worklist"
    );
    assert_eq!(
        envelope_pairs(&b_sends[0].message),
        pairs(&[attestation_entry(&harness, &b_only, slot)])
    );
    assert_eq!(
        selection.signed_batches().len(),
        2,
        "one signing per committee"
    );
}

/// The slot clock stepping back into slot N after N+1's assignments were published, before N's
/// selection deadline, lets N be published again. N's committee was already sent and is not sent
/// again, and N+1's send is unaffected.
#[tokio::test(start_paused = true)]
async fn older_slot_republished_after_a_newer_one_is_not_sent_again() {
    // Arrange
    let harness = one_committee_harness(1);
    let validator = harness.validator_metadata(0, 0);
    let committee = committee_of(&harness, &validator);
    let older = Slot::new(TEST_SLOT);
    let newer = older + 1;

    // Act: publish N at the harness clock and N+1 at the same point in its slot, then step the
    // clock back to that point in N and publish N again.
    publish_and_settle(&harness, attester_assignments(older, [&validator])).await;
    set_clock(&harness, newer, PUBLICATION_IN_SLOT);
    publish_and_settle(&harness, attester_assignments(newer, [&validator])).await;
    set_clock(&harness, older, PUBLICATION_IN_SLOT);
    publish_and_settle(&harness, attester_assignments(older, [&validator])).await;

    // Assert
    let sends = harness.committee_selection.sends();
    let sends_for = |slot| {
        sends
            .iter()
            .filter(|send| {
                send.committee_id == committee && decode_envelope(&send.message).slot == slot
            })
            .count()
    };
    assert_eq!(
        sends_for(older),
        1,
        "the older slot is not sent again after the newer one"
    );
    assert_eq!(sends_for(newer), 1, "the newer slot is sent once");
    assert_eq!(sends.len(), 2);
}

// ==================== Retries and the selection deadline ====================

/// Failed sends are retried, spaced by the retry interval, with the message signed at the start,
/// until one is admitted. The batch is never signed again: identical bytes alone would not show
/// that, since BLS signing is deterministic, so the signings are counted, and the mock tags each
/// signing's message so a re-signed one would differ.
#[tokio::test(start_paused = true)]
async fn failed_sends_retry_the_same_signed_message_until_admitted() {
    // Arrange
    const FAILED_SENDS: usize = 3;
    let harness = one_committee_harness(1);
    let validator = harness.validator_metadata(0, 0);
    let committee = committee_of(&harness, &validator);
    let slot = Slot::new(TEST_SLOT);
    harness.committee_selection.fail_sends(FAILED_SENDS);

    // Act
    publish_and_settle(&harness, attester_assignments(slot, [&validator])).await;

    // Assert
    assert_eq!(
        harness.committee_selection.signed_batches().len(),
        1,
        "the batch is signed once however often its send is retried"
    );
    let sends = harness.committee_selection.sends();
    assert_eq!(sends.len(), FAILED_SENDS + 1);
    let first_message = sends[0].message.as_ssz_bytes();
    for (attempt, send) in sends.iter().enumerate() {
        assert_eq!(
            send.message.as_ssz_bytes(),
            first_message,
            "attempt {attempt} offers the message signed first"
        );
        assert_eq!(send.committee_id, committee);
        assert_eq!(send.admitted, attempt == FAILED_SENDS);
    }
    assert!(
        sends
            .windows(2)
            .all(|pair| pair[1].sent_at - pair[0].sent_at >= RETRY_INTERVAL)
    );
    assert_eq!(
        envelope_pairs(&sends[FAILED_SENDS].message),
        pairs(&[attestation_entry(&harness, &validator, slot)])
    );
}

/// A failed signing is retried until it succeeds, and the result is sent once.
#[tokio::test(start_paused = true)]
async fn failed_signing_is_retried_then_sent_once() {
    // Arrange
    const FAILED_SIGNINGS: usize = 2;
    let harness = one_committee_harness(1);
    let validator = harness.validator_metadata(0, 0);
    harness.committee_selection.fail_signing(FAILED_SIGNINGS);

    // Act
    publish_and_settle(
        &harness,
        attester_assignments(Slot::new(TEST_SLOT), [&validator]),
    )
    .await;

    // Assert
    assert_eq!(
        harness.committee_selection.signed_batches().len(),
        FAILED_SIGNINGS + 1
    );
    let sends = harness.committee_selection.sends();
    assert_eq!(sends.len(), 1);
    assert!(sends[0].admitted);
}

/// Signing that keeps failing is retried, then abandoned at the selection deadline, and nothing
/// is sent.
#[tokio::test(start_paused = true)]
async fn signing_that_keeps_failing_is_abandoned_at_the_deadline() {
    // Arrange
    let harness = one_committee_harness(1);
    let validator = harness.validator_metadata(0, 0);
    harness.committee_selection.fail_signing(usize::MAX);

    // Act
    publish_and_settle(
        &harness,
        attester_assignments(Slot::new(TEST_SLOT), [&validator]),
    )
    .await;
    let signings_past_deadline = harness.committee_selection.signed_batches().len();
    run_past_selection_deadline().await;

    // Assert
    assert!(signings_past_deadline > 1, "failed signing is retried");
    assert_eq!(
        harness.committee_selection.signed_batches().len(),
        signings_past_deadline,
        "signing is not retried after the deadline"
    );
    assert!(harness.committee_selection.sends().is_empty());
}

/// Assignments published at or after the selection deadline start nothing, neither signing nor
/// sending; a millisecond before it they still start.
#[tokio::test(start_paused = true)]
async fn publication_at_or_after_the_selection_deadline_starts_nothing() {
    // (when the assignments are published, how many batches are signed and sent)
    let cases = [
        (SELECTION_DEADLINE_IN_SLOT - Duration::from_millis(1), 1),
        (SELECTION_DEADLINE_IN_SLOT, 0),
        (SELECTION_DEADLINE_IN_SLOT + Duration::from_secs(1), 0),
    ];
    for (published_in_slot, expected) in cases {
        // Arrange
        let harness = one_committee_harness(1);
        let validator = harness.validator_metadata(0, 0);
        let slot = Slot::new(TEST_SLOT);
        set_clock(&harness, slot, published_in_slot);

        // Act
        publish_and_settle(&harness, attester_assignments(slot, [&validator])).await;

        // Assert
        assert_eq!(
            harness.committee_selection.signed_batches().len(),
            expected,
            "published {published_in_slot:?} into the slot"
        );
        assert_eq!(
            harness.committee_selection.sends().len(),
            expected,
            "published {published_in_slot:?} into the slot"
        );
    }
}

/// Sends that keep failing are retried until the selection deadline, and never attempted at or
/// after it.
#[tokio::test(start_paused = true)]
async fn failing_sends_are_retried_until_but_never_at_the_deadline() {
    // Arrange
    let harness = one_committee_harness(1);
    let validator = harness.validator_metadata(0, 0);
    harness.committee_selection.fail_sends(usize::MAX);
    let deadline = Instant::now() + SELECTION_DEADLINE_AFTER_PUBLICATION;

    // Act
    publish_and_settle(
        &harness,
        attester_assignments(Slot::new(TEST_SLOT), [&validator]),
    )
    .await;

    // Assert
    let sends = harness.committee_selection.sends();
    let last = sends.last().expect("the batch send is attempted");
    assert!(sends.iter().all(|send| !send.admitted));
    assert!(
        sends.iter().all(|send| send.sent_at < deadline),
        "no send is attempted at or after the selection deadline"
    );
    assert!(
        last.sent_at + RETRY_INTERVAL >= deadline,
        "the send is retried until the deadline"
    );
    assert_eq!(harness.committee_selection.signed_batches().len(), 1);
}

/// Signing that finishes at or after the selection deadline sends nothing; a millisecond before
/// it the batch is still sent.
#[tokio::test(start_paused = true)]
async fn signing_finished_at_or_after_the_deadline_sends_nothing() {
    // (how long signing takes from publication, how many batches are sent)
    let cases = [
        (
            SELECTION_DEADLINE_AFTER_PUBLICATION - Duration::from_millis(1),
            1,
        ),
        (SELECTION_DEADLINE_AFTER_PUBLICATION, 0),
        (
            SELECTION_DEADLINE_AFTER_PUBLICATION + Duration::from_secs(1),
            0,
        ),
    ];
    for (signing_time, expected_sends) in cases {
        // Arrange
        let harness = one_committee_harness(1);
        let validator = harness.validator_metadata(0, 0);
        harness.committee_selection.delay_signing(signing_time);

        // Act
        publish_and_settle(
            &harness,
            attester_assignments(Slot::new(TEST_SLOT), [&validator]),
        )
        .await;

        // Assert
        assert_eq!(harness.committee_selection.signed_batches().len(), 1);
        assert_eq!(
            harness.committee_selection.sends().len(),
            expected_sends,
            "signing took {signing_time:?}"
        );
    }
}

/// Signing keys that finish loading at or after the selection deadline start no signing and send
/// nothing; a millisecond before it the batch is still signed and sent.
#[tokio::test(start_paused = true)]
async fn keys_loaded_at_or_after_the_deadline_sign_and_send_nothing() {
    // (how long key loading takes from publication, how many batches are signed and sent)
    let cases = [
        (
            SELECTION_DEADLINE_AFTER_PUBLICATION - Duration::from_millis(1),
            1,
        ),
        (SELECTION_DEADLINE_AFTER_PUBLICATION, 0),
        (
            SELECTION_DEADLINE_AFTER_PUBLICATION + Duration::from_secs(1),
            0,
        ),
    ];
    for (loading_time, expected) in cases {
        // Arrange: a share encrypted to the installed operator key, so loading it waits for the
        // held key cache and then succeeds.
        let operator_key = generate_operator_key();
        let validator_key = SecretKey::random();
        let mut committee = create_primary_committee_setup(1);
        committee.set_validator_key(
            0,
            validator_key.public_key().compress(),
            encrypt_share(&operator_key, &validator_key),
        );
        let mut harness = ValidatorStoreTestHarness::new(vec![committee], OUR_OPERATOR_ID);
        install_operator_key(&mut harness, operator_key);
        let validator = harness.validator_metadata(0, 0);
        let release_keys = hold_decrypted_keys(&harness);

        // Act: the execution starts loading at publication and is held there while the clock
        // moves. Paused time does not auto-advance while loading runs, so it finishes
        // `loading_time` after publication.
        harness
            .validator_store
            .update_voting_assignments(attester_assignments(Slot::new(TEST_SLOT), [&validator]));
        tokio::task::yield_now().await;
        tokio::time::advance(loading_time).await;
        drop(release_keys);
        run_past_selection_deadline().await;

        // Assert
        assert_eq!(
            harness.committee_selection.signed_batches().len(),
            expected,
            "keys loaded {loading_time:?} after publication"
        );
        assert_eq!(
            harness.committee_selection.sends().len(),
            expected,
            "keys loaded {loading_time:?} after publication"
        );
    }
}

// ==================== Lighthouse selection callbacks ====================

/// After the committee's batch was sent, Lighthouse's attestation and sync selection callbacks
/// for validators in it only sign locally: nothing is signed for or sent to the network again.
#[tokio::test(start_paused = true)]
async fn late_selection_callbacks_sign_locally_without_sending() {
    // Arrange: validator 0 attests, validator 1 is on sync subnets 0 and 1, and their batch has
    // been sent.
    let harness = one_committee_harness(2);
    let attester = harness.validator_metadata(0, 0);
    let sync = harness.validator_metadata(0, 1);
    let slot = Slot::new(TEST_SLOT);
    let mut assignments = attester_assignments(slot, [&attester]);
    add_sync_duty(&mut assignments, &sync, [(0, 1), (1, 1)]);
    harness
        .validator_store
        .update_voting_assignments(assignments);
    wait_until(|| harness.committee_selection.sends().len() == 1).await;

    // Act
    harness
        .validator_store
        .produce_selection_proof(attester.public_key, slot)
        .await
        .expect("the attestation selection proof is produced");
    harness
        .validator_store
        .produce_sync_selection_proof(&sync.public_key, slot, SyncSubnetId::new(1))
        .await
        .expect("the sync selection proof is produced");
    run_past_selection_deadline().await;

    // Assert
    let calls = harness
        .captured_calls
        .lock()
        .iter()
        .map(|call| {
            (
                call.validator_pubkey,
                call.signing_root,
                matches!(call.requester, SignatureRequester::LocalOnly),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        calls,
        vec![
            (
                attester.public_key,
                attestation_entry(&harness, &attester, slot).2,
                true
            ),
            (
                sync.public_key,
                sync_entry(&harness, &sync, slot, 1).2,
                true
            ),
        ]
    );
    assert_eq!(harness.committee_selection.signed_batches().len(), 1);
    assert_only_the_batch_was_sent(&harness);
}

// ==================== Real collector and processor ====================

/// No Lighthouse selection callback runs. Publishing the slot's assignments must still send one
/// envelope for the committee holding a selection proof for the attester duty and for each
/// distinct sync subnet, however many positions a validator holds on it.
#[tokio::test(flavor = "multi_thread")]
async fn publishing_assignments_sends_complete_batch_without_callbacks() {
    // Arrange: validator 0 attests; validator 1 holds two positions on sync subnet 0 and one on
    // subnet 1.
    let mut harness = one_committee_harness(2);
    let (_collector, mut admitted) = install_recording_collector(&mut harness, OUR_OPERATOR_ID);
    let attester = harness.validator_metadata(0, 0);
    let sync = harness.validator_metadata(0, 1);
    let committee = committee_of(&harness, &attester);
    let slot = Slot::new(TEST_SLOT);
    let mut assignments = attester_assignments(slot, [&attester]);
    add_sync_duty(&mut assignments, &sync, [(0, 2), (1, 1)]);
    let expected = sorted(vec![
        attestation_entry(&harness, &attester, slot),
        sync_entry(&harness, &sync, slot, 0),
        sync_entry(&harness, &sync, slot, 1),
    ]);

    // Act: the slot pipeline publishes the assignments, and nothing else happens.
    harness
        .validator_store
        .update_voting_assignments(assignments);

    // Assert: one committee envelope with exactly the expected validator and root pairs.
    let envelope = tokio::time::timeout(STREAM_TIMEOUT, admitted.recv())
        .await
        .expect("publishing the assignments must send the committee's selection batch by itself")
        .expect("the recorder stays open");
    let message_id = envelope.ssv_message.msg_id();
    assert_eq!(message_id.role(), Some(Role::AggregatorCommittee));
    assert_eq!(
        message_id.duty_executor(),
        Some(DutyExecutor::Committee(committee))
    );
    let batch = decode_envelope(&envelope);
    assert_eq!(
        batch.kind,
        PartialSignatureKind::AggregatorCommitteePartialSig
    );
    assert_eq!(batch.slot, slot);
    assert!(
        batch
            .messages
            .iter()
            .all(|message| message.signer == OUR_OPERATOR_ID)
    );
    assert_eq!(envelope_pairs(&envelope), pairs(&expected));
    assert!(
        tokio::time::timeout(DUPLICATE_GRACE, admitted.recv())
            .await
            .is_err(),
        "a committee sends one selection envelope per slot"
    );
}

/// Real keys, the real collector and a real processor. After the slot pipeline sent the batch,
/// the late attestation and sync callbacks stay pending on this operator's share alone, resolve
/// to the validators' signatures once two peers' shares arrive, and send nothing more.
///
/// Every operator's share of these validators is the validator key itself, a constant sharing
/// polynomial: any threshold of such shares interpolates to the validator's own signature, so
/// reconstruction runs as in production without a key-splitting dependency in this crate.
#[tokio::test(flavor = "multi_thread")]
async fn late_selection_callbacks_resolve_from_peer_shares_without_another_send() {
    // Arrange
    const PEERS: [OperatorId; 2] = [OperatorId(2), OperatorId(3)];
    let operator_key = generate_operator_key();
    let attester_key = SecretKey::random();
    let sync_key = SecretKey::random();
    let mut committee = create_primary_committee_setup(2);
    committee.set_validator_key(
        0,
        attester_key.public_key().compress(),
        encrypt_share(&operator_key, &attester_key),
    );
    committee.set_validator_key(
        1,
        sync_key.public_key().compress(),
        encrypt_share(&operator_key, &sync_key),
    );
    let mut harness = ValidatorStoreTestHarness::new(vec![committee], OUR_OPERATOR_ID);
    install_operator_key(&mut harness, operator_key);
    let (collector, mut admitted) = install_recording_collector(&mut harness, OUR_OPERATOR_ID);
    let attester = harness.validator_metadata(0, 0);
    let sync = harness.validator_metadata(0, 1);
    let slot = Slot::new(TEST_SLOT);
    let attestation = attestation_entry(&harness, &attester, slot);
    let sync_selection = sync_entry(&harness, &sync, slot, 0);
    let mut assignments = attester_assignments(slot, [&attester]);
    add_sync_duty(&mut assignments, &sync, [(0, 1)]);
    harness
        .validator_store
        .update_voting_assignments(assignments);
    let envelope = tokio::time::timeout(STREAM_TIMEOUT, admitted.recv())
        .await
        .expect("the slot pipeline sends the batch")
        .expect("the recorder stays open");
    let sent = decode_envelope(&envelope);
    let (attestation_index, attestation_root) = (ValidatorIndex(attestation.0), attestation.2);
    let (sync_index, sync_root) = (ValidatorIndex(sync_selection.0), sync_selection.2);
    let mut expected_batch = vec![
        (
            attestation_index,
            attestation_root,
            attester_key.sign(attestation_root),
        ),
        (sync_index, sync_root, sync_key.sign(sync_root)),
    ];
    expected_batch.sort_unstable_by_key(|(index, root, _)| (index.0, *root));
    assert_eq!(
        sent.messages
            .iter()
            .map(|message| (
                message.validator_index,
                message.signing_root,
                message.partial_signature.clone()
            ))
            .collect::<Vec<_>>(),
        expected_batch,
        "the batch is signed with the decrypted shares"
    );

    // Act: both callbacks run after the batch was sent, then two peers' shares arrive.
    let store = Arc::clone(&harness.validator_store);
    let attester_pubkey = attester.public_key;
    let attestation_callback =
        tokio::spawn(async move { store.produce_selection_proof(attester_pubkey, slot).await });
    let store = Arc::clone(&harness.validator_store);
    let sync_pubkey = sync.public_key;
    let sync_callback = tokio::spawn(async move {
        store
            .produce_sync_selection_proof(&sync_pubkey, slot, SyncSubnetId::new(0))
            .await
    });
    tokio::time::sleep(OWN_SHARE_ONLY).await;
    assert!(
        !attestation_callback.is_finished() && !sync_callback.is_finished(),
        "this operator's share alone is below the threshold"
    );
    for (index, root, key) in [
        (attestation_index, attestation_root, &attester_key),
        (sync_index, sync_root, &sync_key),
    ] {
        let messages = PEERS
            .iter()
            .map(|&signer| PartialSignatureMessage {
                partial_signature: key.sign(root),
                signing_root: root,
                signer,
                validator_index: index,
            })
            .collect::<Vec<_>>();
        collector
            .receive_partial_signatures(PartialSignatureMessages {
                kind: PartialSignatureKind::AggregatorCommitteePartialSig,
                slot,
                messages: VariableList::new(messages).expect("two peer shares fit"),
            })
            .expect("peer shares enter the processor");
    }

    // Assert
    let attestation_proof = tokio::time::timeout(STREAM_TIMEOUT, attestation_callback)
        .await
        .expect("the attestation callback resolves")
        .expect("the attestation callback task completes")
        .expect("the attestation selection proof is produced");
    assert_eq!(
        attestation_proof,
        SelectionProof::from(attester_key.sign(attestation_root))
    );
    let sync_proof = tokio::time::timeout(STREAM_TIMEOUT, sync_callback)
        .await
        .expect("the sync callback resolves")
        .expect("the sync callback task completes")
        .expect("the sync selection proof is produced");
    assert_eq!(
        sync_proof,
        SyncSelectionProof::from(sync_key.sign(sync_root))
    );
    assert!(
        tokio::time::timeout(DUPLICATE_GRACE, admitted.recv())
            .await
            .is_err(),
        "late callbacks send nothing"
    );
}

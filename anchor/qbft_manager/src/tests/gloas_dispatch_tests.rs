//! `Role::Committee` dispatch tests for the CStar (Gloas) fork.
//!
//! These exercise the fork-gated routing added in #1025 inside
//! `QbftManager::receive_data`: at or after CStar a committee message must spawn
//! a `GloasBeaconVote` instance, before CStar a `BeaconVote` instance. The
//! `DashMap` entry is inserted synchronously inside `get_or_spawn_instance`
//! before the processor task is even dispatched, so map sizes are deterministic
//! immediately after `receive_data` returns.

use fork::ForkSchedule;
use message_sender::testing::MockMessageSender;
use ssv_types::{
    RSA_SIGNATURE_SIZE,
    consensus::{QbftMessage, QbftMessageType},
    message::{MsgType, SSVMessage, SignedSSVMessage},
};
use ssz::Encode;

use super::{setup::setup_test, *};

// Slot picked well past any baseline-epoch boundary so the chosen fork schedule
// dominates routing rather than any genesis edge case (`slot 100` mirrors the
// existing aggregator dispatch test).
const TEST_SLOT_HEIGHT: u64 = 100;

/// Build a single-operator `QbftManager` with the supplied `ForkSchedule`,
/// mirroring the canonical setup from
/// `aggregator_tests::test_aggregator_committee_rejected_before_boole`.
fn build_manager(
    setup: &super::setup::Setup,
    fork_schedule: ForkSchedule,
) -> Arc<QbftManager<types::MainnetEthSpec, ManualSlotClock>> {
    let config = processor::Config {
        max_workers: 4,
        queue_size: Default::default(),
    };
    let senders = processor::spawn(config, setup.executor.clone());
    let (network_tx, _network_rx) = mpsc::unbounded_channel();

    QbftManager::<types::MainnetEthSpec, _>::new(
        senders,
        OperatorId(1).into(),
        setup.clock.clone(),
        Arc::new(MockMessageSender::new(network_tx, OperatorId(1))),
        NonZeroU64::new(32).expect("slots_per_epoch is non-zero"),
        Arc::new(fork_schedule),
    )
    .expect("Manager creation should succeed")
}

/// Build a `Role::Committee` (`SignedSSVMessage`, `QbftMessage`) pair at the
/// given slot height. The shape mirrors the aggregator dispatch test so the
/// two tests stay aligned.
fn build_committee_message(slot_height: u64) -> (SignedSSVMessage, QbftMessage) {
    let msg_id = MessageId::new(
        &DomainType([0; 4]),
        Role::Committee,
        &DutyExecutor::Committee(CommitteeId([0; 32])),
    );

    let qbft_message = QbftMessage {
        qbft_message_type: QbftMessageType::Proposal,
        height: slot_height,
        round: 1,
        identifier: (&msg_id).into(),
        root: Hash256::from([0u8; 32]),
        data_round: 1,
        round_change_justification: ssv_types::VariableList::empty(),
        prepare_justification: ssv_types::VariableList::empty(),
    };

    let ssv_msg = SSVMessage::new(
        MsgType::SSVConsensusMsgType,
        msg_id,
        qbft_message.as_ssz_bytes(),
    )
    .expect("SSVMessage creation should succeed");

    let signed_msg = SignedSSVMessage::new(
        vec![[0xAA; RSA_SIGNATURE_SIZE]],
        vec![OperatorId(1)],
        ssv_msg,
        vec![],
    )
    .expect("SignedSSVMessage creation should succeed");

    (signed_msg, qbft_message)
}

/// With `Fork::CStar` active from genesis, a `Role::Committee` message must
/// spawn a `GloasBeaconVote` instance and leave the legacy `BeaconVote` map
/// untouched.
#[tokio::test]
async fn test_committee_message_routes_to_gloas_at_cstar() {
    // Arrange
    let setup = setup_test(1);
    let manager = build_manager(
        &setup,
        ForkSchedule::new(Fork::CStar, DomainType::default(), "test"),
    );
    let (signed_msg, qbft_message) = build_committee_message(TEST_SLOT_HEIGHT);

    // Act
    let result = manager.receive_data(signed_msg, qbft_message);

    // Assert
    assert!(
        result.is_ok(),
        "receive_data should succeed at CStar, got: {:?}",
        result
    );
    assert_eq!(
        manager.gloas_beacon_vote_instances.len(),
        1,
        "Committee message at CStar must spawn a GloasBeaconVote instance"
    );
    assert_eq!(
        manager.beacon_vote_instances.len(),
        0,
        "Committee message at CStar must NOT spawn a legacy BeaconVote instance"
    );
}

/// Before CStar (here: `Fork::Alan` active from genesis), the existing
/// `BeaconVote` routing must remain in force and the new `GloasBeaconVote` map
/// must stay empty. This is the "BeaconVote unaffected" acceptance criterion.
#[tokio::test]
async fn test_committee_message_routes_to_beacon_pre_cstar() {
    // Arrange
    let setup = setup_test(1);
    let manager = build_manager(
        &setup,
        ForkSchedule::new(Fork::Alan, DomainType::default(), "test"),
    );
    let (signed_msg, qbft_message) = build_committee_message(TEST_SLOT_HEIGHT);

    // Act
    let result = manager.receive_data(signed_msg, qbft_message);

    // Assert
    assert!(
        result.is_ok(),
        "receive_data should succeed pre-CStar, got: {:?}",
        result
    );
    assert_eq!(
        manager.beacon_vote_instances.len(),
        1,
        "Committee message pre-CStar must spawn a BeaconVote instance"
    );
    assert_eq!(
        manager.gloas_beacon_vote_instances.len(),
        0,
        "Committee message pre-CStar must NOT spawn a GloasBeaconVote instance"
    );
}

/// Pin the slot-to-epoch + `active_fork(epoch)` + `>= Fork::CStar` composition
/// at the activation boundary. Off-by-one in any of the three would flip
/// exactly one of the assertions below.
#[tokio::test]
async fn test_committee_message_routes_at_cstar_activation_boundary() {
    use std::collections::BTreeMap;

    use types::Epoch;

    const CSTAR_ACTIVATION_EPOCH: u64 = 5;
    const SLOTS_PER_EPOCH: u64 = 32;

    let setup = setup_test(1);

    let mut configs = BTreeMap::new();
    configs.insert(Fork::Alan, (Epoch::new(0), DomainType::default()));
    configs.insert(
        Fork::CStar,
        (Epoch::new(CSTAR_ACTIVATION_EPOCH), DomainType::default()),
    );
    let schedule = ForkSchedule::from_fork_configs(configs, "test")
        .expect("Alan@0 + CStar@5 is a valid schedule");
    let manager = build_manager(&setup, schedule);

    let last_pre_cstar = CSTAR_ACTIVATION_EPOCH * SLOTS_PER_EPOCH - 1;
    let (signed, qbft) = build_committee_message(last_pre_cstar);
    manager
        .receive_data(signed, qbft)
        .expect("pre-CStar dispatch");
    assert_eq!(manager.beacon_vote_instances.len(), 1);
    assert_eq!(manager.gloas_beacon_vote_instances.len(), 0);

    let first_cstar = CSTAR_ACTIVATION_EPOCH * SLOTS_PER_EPOCH;
    let (signed, qbft) = build_committee_message(first_cstar);
    manager.receive_data(signed, qbft).expect("CStar dispatch");
    assert_eq!(manager.gloas_beacon_vote_instances.len(), 1);
    // BeaconVote map still holds the pre-CStar instance.
    assert_eq!(manager.beacon_vote_instances.len(), 1);
}

use super::{setup::setup_test, *};

/// Test that PTCCommittee messages are rejected before the CStar fork.
/// Critical security test: the role must not be processed until CStar is active.
#[tokio::test]
async fn test_ptc_committee_rejected_before_cstar() {
    use fork::ForkSchedule;
    use message_sender::testing::MockMessageSender;
    use ssv_types::{
        RSA_SIGNATURE_SIZE,
        consensus::{QbftMessage, QbftMessageType},
        message::{MsgType, SSVMessage, SignedSSVMessage},
    };
    use ssz::Encode;

    let setup = setup_test(1);

    // Create QbftManager with fork schedule pinned at Boole (no CStar)
    let config = processor::Config {
        max_workers: 4,
        queue_size: Default::default(),
    };
    let senders = processor::spawn(config, setup.executor);
    let (network_tx, _network_rx) = mpsc::unbounded_channel();

    let manager = QbftManager::<types::MainnetEthSpec, _>::new(
        senders,
        OperatorId(1).into(),
        setup.clock,
        Arc::new(MockMessageSender::new(network_tx, OperatorId(1))),
        NonZeroU64::new(32).expect("slots_per_epoch is non-zero"),
        Arc::new(ForkSchedule::new(
            Fork::Boole,
            DomainType::default(),
            "test",
        )), // No CStar fork
    )
    .expect("Manager creation should succeed");

    // Create a PTCCommittee message
    let msg_id = MessageId::new(
        &DomainType([0; 4]),
        Role::PTCCommittee,
        &DutyExecutor::Committee(CommitteeId([0; 32])),
    );

    let qbft_message = QbftMessage {
        qbft_message_type: QbftMessageType::Proposal,
        height: 100, // Slot 100, well before any CStar epoch
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

    // Call receive_data - should return RoleNotActive
    let result = manager.receive_data(signed_msg, qbft_message);

    assert!(
        matches!(result, Err(QbftError::RoleNotActive)),
        "Expected RoleNotActive error before CStar fork, got: {:?}",
        result
    );
}

/// Test that PTCCommittee messages are accepted after the CStar fork.
/// Verifies the fork gating allows messages through when CStar is active.
#[tokio::test]
async fn test_ptc_committee_accepted_after_cstar() {
    use fork::ForkSchedule;
    use message_sender::testing::MockMessageSender;
    use ssv_types::{
        RSA_SIGNATURE_SIZE,
        consensus::{QbftMessage, QbftMessageType},
        message::{MsgType, SSVMessage, SignedSSVMessage},
    };
    use ssz::Encode;

    let setup = setup_test(1);

    // Create fork schedule with CStar active at epoch 0
    let fork_schedule = ForkSchedule::new(Fork::CStar, DomainType::default(), "test");

    let config = processor::Config {
        max_workers: 4,
        queue_size: Default::default(),
    };
    let senders = processor::spawn(config, setup.executor);
    let (network_tx, _network_rx) = mpsc::unbounded_channel();

    let manager = QbftManager::<types::MainnetEthSpec, _>::new(
        senders,
        OperatorId(1).into(),
        setup.clock,
        Arc::new(MockMessageSender::new(network_tx, OperatorId(1))),
        NonZeroU64::new(32).expect("slots_per_epoch is non-zero"),
        Arc::new(fork_schedule),
    )
    .expect("Manager creation should succeed");

    // Create a PTCCommittee message
    let msg_id = MessageId::new(
        &DomainType([0; 4]),
        Role::PTCCommittee,
        &DutyExecutor::Committee(CommitteeId([0; 32])),
    );

    let qbft_message = QbftMessage {
        qbft_message_type: QbftMessageType::Proposal,
        height: 100, // Any slot, CStar is active from epoch 0
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

    // Call receive_data - should NOT return RoleNotActive
    let result = manager.receive_data(signed_msg, qbft_message);

    // It might return Ok or some other error (e.g., no instance running),
    // but critically it should NOT be RoleNotActive
    assert!(
        !matches!(result, Err(QbftError::RoleNotActive)),
        "Should not return RoleNotActive after CStar fork, got: {:?}",
        result
    );
}

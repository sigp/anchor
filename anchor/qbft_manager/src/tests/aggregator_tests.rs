use super::{setup::setup_test, *};

/// Test that AggregatorCommittee messages are rejected before the Boole fork.
/// This is a critical security test - the role should not be processed until Boole is active.
#[tokio::test]
async fn test_aggregator_committee_rejected_before_boole() {
    use fork::ForkSchedule;
    use message_sender::testing::MockMessageSender;
    use ssv_types::{
        RSA_SIGNATURE_SIZE,
        consensus::{QbftMessage, QbftMessageType},
        message::{MsgType, SSVMessage, SignedSSVMessage},
    };
    use ssz::Encode;

    let setup = setup_test(1);

    // Create QbftManager with default fork schedule (no Boole)
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
        Arc::new(ForkSchedule::new(Fork::Alan, DomainType::default(), "test")), // No Boole fork
    )
    .expect("Manager creation should succeed");

    // Create an AggregatorCommittee message
    let msg_id = MessageId::new(
        &DomainType([0; 4]),
        Role::AggregatorCommittee,
        &DutyExecutor::Committee(CommitteeId([0; 32])),
    );

    let qbft_message = QbftMessage {
        qbft_message_type: QbftMessageType::Proposal,
        height: 100, // Slot 100, well before any Boole epoch
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
        "Expected RoleNotActive error before Boole fork, got: {:?}",
        result
    );
}

use super::{setup::setup_test, *};

/// Test that AggregatorCommittee messages are rejected before the Boole fork.
/// This is a critical security test - the role should not be processed until Boole is active.
#[tokio::test]
async fn test_aggregator_committee_rejected_before_boole() {
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
        Arc::new(types::ChainSpec::mainnet()), // Gloas not scheduled
    )
    .expect("Manager creation should succeed");

    // Create an AggregatorCommittee message at slot 100, well before any Boole epoch.
    let (signed_msg, qbft_message) = build_signed_consensus_pair(
        Role::AggregatorCommittee,
        &DutyExecutor::Committee(CommitteeId([0; 32])),
        QbftMessageType::Proposal,
        100,
    );

    let result =
        manager.receive_network_message(signed_msg, qbft_message, unexpected_proposer_duty_lookup);

    assert!(
        matches!(result, Err(QbftError::RoleNotActive)),
        "Expected RoleNotActive error before Boole fork, got: {:?}",
        result
    );
}

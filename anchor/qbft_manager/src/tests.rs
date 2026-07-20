use std::{
    collections::HashMap,
    num::NonZeroU64,
    sync::{Arc, LazyLock, RwLock, RwLockWriteGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use fork::{Fork, ForkSchedule};
use message_sender::testing::MockMessageSender;
use processor::Senders;
use qbft::InstanceHeight;
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::{
    Cluster, ClusterId, CommitteeId, IndexSet, OperatorId,
    consensus::{BeaconVote, NoDataValidation, QbftMessage, QbftMessageType},
    domain_type::DomainType,
    get_f,
    message::SignedSSVMessage,
    msgid::{DutyExecutor, MessageId, Role},
    quorum_size,
};
use ssz::Decode;
use task_executor::{ShutdownReason, TaskExecutor};
use tokio::{
    pin, select,
    sync::{
        mpsc,
        mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
        oneshot,
    },
    time::{Instant, sleep},
};
use tracing::{debug, error};
use types::{EthSpec, Hash256, Slot};

use super::{
    CommitteeInstanceId, Completed, QbftDecidable, QbftError, QbftInitialization, QbftManager,
    QbftMessageKind, TimeoutMode, WrappedQbftMessage,
};
use crate::instance::qbft_instance;

mod aggregator_tests;
mod gloas_dispatch_tests;
mod setup;
mod timeout_tests;

/// The time we wait at most for consensus results until the test times out. Note that this is not
/// real time, but simulated time, if the test is started with `start_paused = true`
pub const TEST_TIMEOUT: Duration = Duration::from_secs(300);

// Init tracing
static TRACING: LazyLock<()> = LazyLock::new(|| {
    let env_filter = tracing_subscriber::EnvFilter::new("debug");
    tracing_subscriber::fmt().with_env_filter(env_filter).init();
});

// Top level Testing Context to provide clean wrapper around testing framework
pub struct TestContext<E, D>
where
    E: EthSpec,
    D: QbftDecidable<E>,
    D::Id: Send + Sync + Clone,
{
    pub tester: Arc<QbftTester<E, D>>,
    pub consensus_rx: UnboundedReceiver<ConsensusResult>,
}

impl<E, D> TestContext<E, D>
where
    E: EthSpec,
    D: QbftDecidable<E>,
    D::Id: Send + Sync + Clone,
{
    // Create a new test context with default setup
    pub async fn new(
        clock: ManualSlotClock,
        executor: TaskExecutor,
        size: CommitteeSize,
        test_data: Vec<(D, D::Id)>,
    ) -> Self {
        Self::new_with_delays(clock, executor, size, test_data, HashMap::new()).await
    }

    pub async fn new_with_delays(
        clock: ManualSlotClock,
        executor: TaskExecutor,
        size: CommitteeSize,
        test_data: Vec<(D, D::Id)>,
        delay_initialization: HashMap<OperatorId, Duration>,
    ) -> Self {
        let (mut tester, network_rx) = QbftTester::new(clock, executor, size);
        let result_rx = tester.start_instance(test_data, delay_initialization).await;
        let (consensus_tx, consensus_rx) = mpsc::unbounded_channel();

        let tester = Arc::new(tester);
        let tester_clone = tester.clone();

        // Run the instance in background
        tokio::spawn(async move {
            tester_clone
                .run_until_complete(network_rx, result_rx, consensus_tx)
                .await;
        });

        Self {
            tester,
            consensus_rx,
        }
    }

    // Helper to set multiple operators offline
    pub fn set_operators_offline(&self, operator_ids: &[u64]) {
        for id in operator_ids {
            self.tester.modify_behavior(OperatorId(*id)).set_offline();
        }
    }

    // Helper to set multiple operators online
    pub fn set_operators_online(&self, operator_ids: &[u64]) {
        for id in operator_ids {
            self.tester.modify_behavior(OperatorId(*id)).set_online();
        }
    }

    // Helper to set byzantine behavior for multiple operators
    pub fn set_operators_byzantine(&self, operator_ids: &[u64], behavior: ByzantineBehavior) {
        for id in operator_ids {
            self.tester
                .modify_behavior(OperatorId(*id))
                .set_byzantine(behavior);
        }
    }

    // Helper to verify consensus is reached
    pub async fn verify_consensus(&mut self) {
        // Track whether we got any consensus result at all
        let mut got_any_result = false;

        // Timeout after a while to avoid hanging forever
        let timeout = sleep(TEST_TIMEOUT);
        pin!(timeout);

        // Receive in a loop until the channel is closed.
        loop {
            select! {
                result = self.consensus_rx.recv() => {
                    let Some(result) = result else {
                        // channel closed
                        break;
                    };

                    got_any_result = true;

                    // Confirm that consensus was reached
                    assert!(result.reached_consensus, "Consensus was not reached");

                    // Confirm that the aggregated message contains a quorum of signatures
                    let aggregated_commit = result
                        .aggregated_commit
                        .expect("If consensus was reached, this must exist");
                    assert!(
                        aggregated_commit.signatures().len() as u64
                            >= quorum_size(self.tester.size as usize) as u64
                    );
                },
                _ = &mut timeout => {
                    panic!("test timed out")
                }
            }
        }

        // At this point the channel has closed, so if we never received anything, fail the test.
        assert!(
            got_any_result,
            "verify_consensus: no consensus result was ever returned"
        );
    }
}

// The only allowed qbft committee sizes
#[derive(Debug, Copy, Clone)]
pub enum CommitteeSize {
    Four = 4,
    Seven = 7,
    Ten = 10,
    Thirteen = 13,
}

/// The main test coordinator that manages multiple QBFT instances
pub struct QbftTester<E, D>
where
    E: EthSpec,
    D: QbftDecidable<E>,
    D::Id: Send + Sync + Clone,
{
    // Senders to the processor
    senders: Senders,
    // Track mapping from operator id to the respective manager
    managers: HashMap<OperatorId, Arc<QbftManager<E, ManualSlotClock>>>,
    // The size of the committee
    pub size: CommitteeSize,
    // Mapping of the data hash to the data identifier. This is to send data to the proper instance
    identifiers: HashMap<u64, D::Id>,
    // Mapping from data to the results of the consensus
    results: RwLock<HashMap<Hash256, ConsensusResult>>,
    // The number of individual qbft instances that are running at any given moment
    num_running: RwLock<HashMap<Hash256, u64>>,
    // Specific behavior for each operator on how they should behave during an instance
    behavior: HashMap<OperatorId, Arc<RwLock<OperatorBehavior>>>,
    // Cluster that all instances use
    cluster: Cluster,
}

#[derive(Clone, Debug, PartialEq, Default, Copy)]
pub enum OperationalStatus {
    #[default]
    Online,
    Offline,
}

#[derive(Clone, Debug, PartialEq, Default, Copy)]
pub enum ByzantineBehavior {
    #[default]
    None,
    // Send conflicting votes for the same round
    DoubleVote,
    // Drop all messages of certain types
    MessageSuppression(QbftMessageType),
    // Modify the round so that the message is invalid
    InvalidMessage,
}

// Describes the behavior of an operator
#[derive(Clone, Debug, Default, Copy)]
pub struct OperatorBehavior {
    // Operational behavior
    pub status: OperationalStatus,
    // Byzantine behavior of the node
    pub byzantine: ByzantineBehavior,
}

impl OperatorBehavior {
    pub fn new() -> Self {
        Self {
            status: OperationalStatus::Online,
            byzantine: ByzantineBehavior::None,
        }
    }

    // Set this node as offline
    pub fn set_offline(&mut self) {
        self.status = OperationalStatus::Offline;
    }

    // Set this node online, aka noraml behavior
    pub fn set_online(&mut self) {
        self.status = OperationalStatus::Online;
    }

    // Check if this node is offline
    fn is_offline(&self) -> bool {
        self.status == OperationalStatus::Offline
    }

    // Set the byzantine behavior of the node
    pub fn set_byzantine(&mut self, behavior: ByzantineBehavior) {
        self.byzantine = behavior;
    }
}

impl<E, D> QbftTester<E, D>
where
    E: EthSpec,
    D: QbftDecidable<E> + 'static,
    D::Id: Send + Sync + Clone,
{
    /// Create a new QBFT tester instance
    pub fn new(
        slot_clock: ManualSlotClock,
        executor: TaskExecutor,
        size: CommitteeSize,
    ) -> (Self, mpsc::UnboundedReceiver<SignedSSVMessage>) {
        // Setup the processor
        let config = processor::Config {
            max_workers: 15,
            queue_size: Default::default(),
        };
        let sender_queues = processor::spawn(config, executor);

        // Simulate the network sender and receiver. Qbft instances will send UnsignedSSVMessages
        // out on the network_tx and they will be received by the network_rx to be "signed" and then
        // broadcasted back into the instances
        let (network_tx, network_rx) = mpsc::unbounded_channel();

        // Create a test fork schedule with a default domain type
        let fork_schedule = Arc::new(ForkSchedule::new(Fork::Alan, DomainType::default(), "test"));
        // Gloas not scheduled: committee consensus uses the legacy BeaconVote path.
        let spec = Arc::new(types::ChainSpec::mainnet());

        // Construct and save a manager for each operator in the committee. By having access to all
        // the managers in the committee, we can direct messages to the proper place and
        // spawn multiple concurrent instances
        let mut managers = HashMap::new();
        let mut behavior = HashMap::new();
        for id in 1..=(size as u64) {
            let operator_id = OperatorId(id);
            let manager = QbftManager::new(
                sender_queues.clone(),
                operator_id.into(),
                slot_clock.clone(),
                Arc::new(MockMessageSender::new(network_tx.clone(), operator_id)),
                NonZeroU64::new(32).expect("slots_per_epoch is non-zero"),
                fork_schedule.clone(),
                spec.clone(),
            )
            .expect("Creation should not fail");

            managers.insert(operator_id, manager);

            behavior.insert(operator_id, Arc::new(RwLock::new(OperatorBehavior::new())));
        }

        // Dummy cluster
        let cluster = Cluster {
            cluster_id: ClusterId([0; 32]),
            owner: Default::default(),
            fee_recipient: Default::default(),
            liquidated: false,
            cluster_members: (1..=(size as u64)).map(OperatorId).collect(),
        };

        (
            Self {
                senders: sender_queues,
                identifiers: HashMap::new(),
                managers,
                size,
                results: RwLock::new(HashMap::new()),
                num_running: RwLock::new(HashMap::new()),
                cluster,
                behavior,
            },
            network_rx,
        )
    }

    // Start a new full test instance for the provided configuration. This will start a new qbft
    // instance for each operator in the committee. This simulates distributed instances each
    // starting their own qbft instance when they must reach consensus with the rest of the
    // committee
    pub async fn start_instance(
        &mut self,
        all_data: Vec<(D, D::Id)>,
        delay_initialization: HashMap<OperatorId, Duration>,
    ) -> UnboundedReceiver<(Hash256, Result<Completed<D>, QbftError>)> {
        let (result_tx, result_rx) = mpsc::unbounded_channel();

        // All operators share the same round-deadline origin, mirroring production where the origin
        // is derived from the slot clock rather than each operator's own initialization
        // time. Operators initialized with a delay start with (partially) expired round
        // deadlines and must catch up to the committee's round cadence.
        let round_deadline_origin = Instant::now();

        for (data, data_id) in all_data {
            let height = *data.instance_height(&data_id) as u64;
            self.identifiers.insert(height, data_id.clone());

            // Track the consensus results
            let min_for_consensus = quorum_size(self.size as usize) as u64;
            self.results.write().unwrap().insert(
                data.hash(),
                ConsensusResult {
                    min_for_consensus,
                    ..Default::default()
                },
            );

            // Record that we have self.size instances running
            self.num_running
                .write()
                .unwrap()
                .insert(data.hash(), self.size as u64);

            // Go through all of the managers. Spawn a new instance for the data and record it
            for (operator, manager) in self.managers.iter() {
                let manager_clone = manager.clone();
                let cluster = self.cluster.clone();
                let data_clone = data.clone();
                let id_clone = data_id.clone();
                let tx_clone = result_tx.clone();
                let delay_initialization = delay_initialization
                    .get(operator)
                    .copied()
                    .unwrap_or(Duration::ZERO);

                // decide the instance
                let _ = self.senders.permitless.send_async(
                    async move {
                        // Wait for initialization delay
                        sleep(delay_initialization).await;
                        // Operator is online, start the instance
                        let result = manager_clone
                            .decide_instance(
                                id_clone,
                                data_clone.clone(),
                                Box::new(NoDataValidation),
                                TimeoutMode::SlotTime {
                                    round_deadline_origin,
                                },
                                &cluster.cluster_members,
                            )
                            .await;
                        let _ = tx_clone.send((data_clone.hash(), result));
                    },
                    "qbft_tests",
                );
            }
        }
        result_rx
    }

    // Get a write lock to the behavior so that we can modify it while the instance is running
    fn modify_behavior(&self, id: OperatorId) -> RwLockWriteGuard<'_, OperatorBehavior> {
        self.behavior
            .get(&id)
            .expect("Value Exist")
            .write()
            .expect("Value Exist")
    }

    // Get the behavior for the operator
    fn get_behavior(&self, id: &OperatorId) -> Arc<RwLock<OperatorBehavior>> {
        self.behavior.get(id).expect("Value Exists").clone()
    }

    // When all the instances are spawned, handle all outgoing messages
    async fn run_until_complete(
        &self,
        mut network_rx: mpsc::UnboundedReceiver<SignedSSVMessage>,
        mut result_rx: UnboundedReceiver<(Hash256, Result<Completed<D>, QbftError>)>,
        consensus_tx: UnboundedSender<ConsensusResult>,
    ) {
        loop {
            tokio::select! {
                maybe_signed = network_rx.recv() => {
                    match maybe_signed {
                        Some(signed) => {
                            // We have a signed ssv message. The next step is to then broadcast this onto
                            // the network. Here, we will just mock this now being received by all of the
                            // other instances
                            let wrapped = self.signed_to_wrapped(signed);
                            self.process_network_message(wrapped);
                        },
                        None => {
                            debug!("network_rx is closed, exiting loop");
                            break;
                        }
                    }
                }
                maybe_result = result_rx.recv() => {
                    match maybe_result {
                        Some((hash, completion)) => {
                            self.handle_completion(hash, completion);
                        }
                        None => {
                            debug!("result_rx is closed, exiting loop");
                            break;
                        }
                    }
                }
            }

            if self.finished() {
                for res in self.results.read().unwrap().values().cloned() {
                    let _ = consensus_tx.send(res);
                }
                break;
            }
        }
        // drop so the consensus receiver gets a close notification
        drop(consensus_tx);
    }

    // Once an instance has completed, we want to record what happened
    fn handle_completion(&self, hash: Hash256, msg: Result<Completed<D>, QbftError>) {
        // Decrement the amount of instances running for this data
        let mut num_running_write = self.num_running.write().unwrap();
        let num = num_running_write.get_mut(&hash).expect("Value exists");
        *num -= 1;

        let mut results_write = self.results.write().unwrap();
        let results = results_write.get_mut(&hash).expect("Value exists");
        match msg {
            Ok(completed) => match completed {
                Completed::Success(_) => {
                    results.successful += 1;

                    // Check if we have reached consensus
                    if results.successful >= results.min_for_consensus {
                        results.reached_consensus = true;
                    }
                }
                Completed::TimedOut => {
                    results.timed_out += 1;
                }
            },
            Err(e) => {
                // Just log the error
                error!("{:?}", e);
            }
        }
    }

    // Check if all of the instances have finished running
    fn finished(&self) -> bool {
        let mut finished = true;
        // Make sure there are no more running instances
        for running in self.num_running.read().unwrap().values() {
            finished &= *running <= get_f(self.size as usize) as u64;
        }

        // Make sure we have received all of the aggregated commit message. There is race condition
        // where we get marked as finished and try to verify consensus while we have not yet
        // processed the final message
        for results in self.results.read().unwrap().values() {
            finished &= results.aggregated_commit.is_some();
        }

        finished
    }

    // Convert a signed ssv message into a wrapped ssv message
    fn signed_to_wrapped(&self, signed: SignedSSVMessage) -> WrappedQbftMessage {
        let deser_qbft = QbftMessage::from_ssz_bytes(signed.ssv_message().data())
            .expect("We have a valid qbft message");
        WrappedQbftMessage {
            signed_message: signed,
            qbft_message: deser_qbft,
        }
    }

    // Process and send a network message to the correct instance
    fn process_network_message(&self, mut wrapped_msg: WrappedQbftMessage) {
        let sender_operator_id = wrapped_msg
            .signed_message
            .operator_ids()
            .first()
            .expect("One signer");

        // If this is a decided message, want to record it in the consensus results.
        // We know this is an aggregated commit if the number of signatures is > 1
        if wrapped_msg.signed_message.signatures().len() > 1 {
            let mut results_write = self.results.write().unwrap();
            let results = results_write
                .get_mut(&wrapped_msg.qbft_message.root)
                .expect("Value exists");
            results.aggregated_commit = Some(wrapped_msg.signed_message);
            return;
        }

        // Now we have a message ready to be sent back into the instance. Get the id
        // corresponding to the message.
        let data_id = self
            .identifiers
            .get(&wrapped_msg.qbft_message.height)
            .expect("Value exists");

        // Check the sender behavior
        let sender_behavior = self.get_behavior(sender_operator_id);
        let sender_read = sender_behavior.read().expect("Exists");
        if sender_read.is_offline() {
            return;
        }

        // Check for byzantine behavior where we should ignore this message
        if !self.should_process_message(&wrapped_msg, &sender_read.byzantine) {
            return;
        }

        // Check for byzantine behavior where we should modify the message/send more
        let messages = self.modify_for_byzantine(&mut wrapped_msg, &sender_read.byzantine);

        // for each operator, send the message to the instance for the data
        for id in 1..=(self.size as u64) {
            let operator_id = OperatorId::from(id);
            let manager = self.managers.get(&operator_id).unwrap().clone();

            // Check the receive behavior
            let receiver_behavior = self.get_behavior(&operator_id);
            let receiver_read = receiver_behavior.read().expect("Exists");
            if receiver_read.is_offline() {
                continue;
            }

            for message in &messages {
                let _ = manager.pass_to_instance::<D>(data_id.clone(), message.clone());
            }
        }
    }

    fn should_process_message(
        &self,
        msg: &WrappedQbftMessage,
        behavior: &ByzantineBehavior,
    ) -> bool {
        let wrapped_msg_type = msg.qbft_message.qbft_message_type;
        match behavior {
            ByzantineBehavior::MessageSuppression(msg_type) => wrapped_msg_type != *msg_type,
            _ => true,
        }
    }

    // Check the behavior of the sender for byzantine behavior. If so, adjust the message
    // accordingly
    fn modify_for_byzantine(
        &self,
        msg: &mut WrappedQbftMessage,
        behavior: &ByzantineBehavior,
    ) -> Vec<WrappedQbftMessage> {
        match behavior {
            ByzantineBehavior::DoubleVote => vec![msg.clone(), msg.clone()],
            ByzantineBehavior::InvalidMessage => {
                msg.qbft_message.round = u64::MAX;
                vec![msg.clone()]
            }
            _ => vec![msg.clone()],
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct ConsensusResult {
    reached_consensus: bool,
    min_for_consensus: u64,
    successful: u64,
    timed_out: u64,
    aggregated_commit: Option<SignedSSVMessage>,
}

#[cfg(test)]
mod manager_tests {
    use super::{setup::setup_test, *};

    #[tokio::test]
    // Test running a single instance and confirm that it reaches consensus
    async fn test_basic_run() {
        let setup = setup_test(1);
        let mut context = TestContext::<types::MainnetEthSpec, BeaconVote>::new(
            setup.clock,
            setup.executor,
            CommitteeSize::Four,
            setup.all_data,
        )
        .await;

        context.verify_consensus().await;
    }

    #[tokio::test]
    // Take the leader offline to test a round change
    async fn test_round_change() {
        let setup = setup_test(1);
        let mut context = TestContext::<types::MainnetEthSpec, BeaconVote>::new(
            setup.clock,
            setup.executor,
            CommitteeSize::Four,
            setup.all_data,
        )
        .await;

        context.set_operators_offline(&[2]);
        context.verify_consensus().await;
    }

    #[tokio::test]
    // Test one offline operator
    async fn test_fault_operator() {
        let setup = setup_test(1);
        let mut context = TestContext::<types::MainnetEthSpec, BeaconVote>::new(
            setup.clock,
            setup.executor,
            CommitteeSize::Four,
            setup.all_data,
        )
        .await;

        context.set_operators_offline(&[1]);
        context.verify_consensus().await;
    }

    #[tokio::test]
    // Go through all committee sizes and confirm that we can reach consensus with f faulty
    async fn test_consensus_f_faulty() {
        let setup = setup_test(1);
        let sizes = vec![
            (CommitteeSize::Four, vec![1]),
            (CommitteeSize::Seven, vec![1, 3]),
            (CommitteeSize::Ten, vec![1, 3, 4]),
            (CommitteeSize::Thirteen, vec![1, 3, 4, 5]),
        ];

        for (size, faulty) in sizes {
            let mut context = TestContext::<types::MainnetEthSpec, BeaconVote>::new(
                setup.clock.clone(),
                setup.executor.clone(),
                size,
                setup.all_data.clone(),
            )
            .await;

            context.set_operators_offline(&faulty);
            context.verify_consensus().await;
        }
    }

    #[tokio::test]
    // Test running concurrent instances and confirm that they reach consensus
    async fn test_concurrent_runs() {
        let setup = setup_test(2);
        let mut context = TestContext::<types::MainnetEthSpec, BeaconVote>::new(
            setup.clock,
            setup.executor,
            CommitteeSize::Four,
            setup.all_data,
        )
        .await;

        context.verify_consensus().await;
    }

    #[tokio::test(start_paused = true)]
    // Start with > f fault and then recover them. This should reach consensus
    async fn test_recovery() {
        let setup = setup_test(1);
        let mut context = TestContext::<types::MainnetEthSpec, BeaconVote>::new(
            setup.clock,
            setup.executor,
            CommitteeSize::Four,
            setup.all_data,
        )
        .await;

        context.set_operators_offline(&[1, 2]);

        tokio::time::sleep(Duration::from_secs(3)).await;
        context.set_operators_online(&[1, 2]);

        context.verify_consensus().await;
    }

    #[tokio::test]
    // Test commit message suppression for an operator
    async fn test_commit_suppression() {
        let setup = setup_test(1);
        let mut context = TestContext::<types::MainnetEthSpec, BeaconVote>::new(
            setup.clock,
            setup.executor,
            CommitteeSize::Four,
            setup.all_data,
        )
        .await;

        context.set_operators_byzantine(
            &[1],
            ByzantineBehavior::MessageSuppression(QbftMessageType::Commit),
        );
        context.verify_consensus().await;
    }

    #[tokio::test]
    // Test sending double messages
    async fn test_send_double() {
        let setup = setup_test(1);
        let mut context = TestContext::<types::MainnetEthSpec, BeaconVote>::new(
            setup.clock,
            setup.executor,
            CommitteeSize::Four,
            setup.all_data,
        )
        .await;

        context.set_operators_byzantine(&[1], ByzantineBehavior::DoubleVote);
        context.verify_consensus().await;
    }

    #[tokio::test]
    // Test one of the nodes sending invalid messages
    async fn test_invalid_message() {
        let setup = setup_test(1);
        let mut context = TestContext::<types::MainnetEthSpec, BeaconVote>::new(
            setup.clock,
            setup.executor,
            CommitteeSize::Four,
            setup.all_data,
        )
        .await;

        context.set_operators_byzantine(&[1], ByzantineBehavior::InvalidMessage);
        context.verify_consensus().await;
    }

    #[tokio::test(start_paused = true)]
    // Test network partition scenarios
    // This simulates temporary network partitions by taking nodes offline and bringing them back
    async fn test_network_partition() {
        let setup = setup_test(1);
        let mut context = TestContext::<types::MainnetEthSpec, BeaconVote>::new(
            setup.clock,
            setup.executor,
            CommitteeSize::Ten, // Using larger committee for partition testing
            setup.all_data,
        )
        .await;

        // Initial partition. We have > f offline so we will not be able to reach consensus
        context.set_operators_offline(&[3, 4, 5, 6]);

        // Wait and change partition
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Bring original back online, and then take = f offline. Should be able to reach consensus
        // now
        context.set_operators_online(&[3, 4, 5, 6]);
        context.set_operators_offline(&[6, 7, 8]);

        context.verify_consensus().await;
    }

    #[tokio::test(start_paused = true)]
    // Test two instances starting late.
    // If an instance starts late, it needs to catch up properly, including round changes.
    // In this test, there are two nodes offline at the start, with one coming back in the second
    // round, and the other one coming back in the third round.
    // We assert that the instance can finish.
    // This is different compared to network partition because here, messages are delayed instead
    // of dropped.
    async fn test_late_initialization() {
        let setup = setup_test(1);

        let initialization_delays = HashMap::from([
            (OperatorId(2), Duration::from_secs(3)), // Middle of round 2
            (OperatorId(3), Duration::from_secs(5)), // Middle of round 3
        ]);

        let mut context = TestContext::<types::MainnetEthSpec, BeaconVote>::new_with_delays(
            setup.clock,
            setup.executor,
            CommitteeSize::Four,
            setup.all_data,
            initialization_delays,
        )
        .await;

        context.verify_consensus().await;
    }

    #[tokio::test(start_paused = true)]
    // Test operators initializing at different times against a shared round-deadline origin.
    // All operators measure their round deadlines from the same instant, so late initializers
    // start with already-expired round deadlines and rejoin the committee's cadence (the
    // cascade itself is pinned by `test_slottime_late_init_cascades_round_changes`).
    // Consensus must still be reached.
    async fn test_slottime_init_skew_convergence() {
        let setup = setup_test(1);

        let initialization_delays = HashMap::from([
            (OperatorId(2), Duration::from_secs(1)), // Middle of round 1
            (OperatorId(3), Duration::from_secs(3)), // Middle of round 2
            (OperatorId(4), Duration::from_secs(5)), // Middle of round 3
        ]);

        let mut context = TestContext::<types::MainnetEthSpec, BeaconVote>::new_with_delays(
            setup.clock,
            setup.executor,
            CommitteeSize::Four,
            setup.all_data,
            initialization_delays,
        )
        .await;

        context.verify_consensus().await;
    }

    /// Test that AggregatorCommittee messages are accepted after the Boole fork.
    /// Verifies the fork gating allows messages through when Boole is active.
    #[tokio::test]
    async fn test_aggregator_committee_accepted_after_boole() {
        use fork::{Fork, ForkSchedule};
        use message_sender::testing::MockMessageSender;
        use ssv_types::{
            RSA_SIGNATURE_SIZE,
            consensus::{QbftMessage, QbftMessageType},
            message::{MsgType, SSVMessage, SignedSSVMessage},
        };
        use ssz::Encode;

        let setup = setup_test(1);

        // Create fork schedule with Boole active at epoch 0
        let fork_schedule = ForkSchedule::new(Fork::Boole, DomainType::default(), "test");

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
            Arc::new(types::ChainSpec::mainnet()),
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
            height: 100, // Any slot, Boole is active from epoch 0
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
            "Should not return RoleNotActive after Boole fork, got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn envelope_proposer_validator_executor_is_transient_role_not_active() {
        // Validator-executor EnvelopeProposer is a correctly-formed message whose QBFT
        // routing is not wired yet (PR4/#1122): transient RoleNotActive, NOT InconsistentMessageId.
        use fork::{Fork, ForkSchedule};
        use message_sender::testing::MockMessageSender;
        use ssv_types::{
            RSA_SIGNATURE_SIZE,
            consensus::{QbftMessage, QbftMessageType},
            message::{MsgType, SSVMessage, SignedSSVMessage},
        };
        use ssz::Encode;

        let setup = setup_test(1);

        // Create fork schedule with Boole active (latest SSV fork)
        // EnvelopeProposer requires Ethereum's Gloas (ePBS) fork, which mainnet ChainSpec has
        let fork_schedule = ForkSchedule::new(Fork::Boole, DomainType::default(), "test");

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
            Arc::new(types::ChainSpec::mainnet()),
        )
        .expect("Manager creation should succeed");

        // Create an EnvelopeProposer message with Validator executor
        let msg_id = MessageId::new(
            &DomainType([0; 4]),
            Role::EnvelopeProposer,
            &DutyExecutor::Validator(bls::PublicKeyBytes::empty()),
        );

        let qbft_message = QbftMessage {
            qbft_message_type: QbftMessageType::Proposal,
            height: 100, // Any slot in Gloas fork
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

        // Act: Call receive_data
        let result = manager.receive_data(signed_msg, qbft_message);

        // Assert: Must be transient RoleNotActive (routing not wired), NOT InconsistentMessageId
        assert!(
            matches!(result, Err(QbftError::RoleNotActive)),
            "validator-executor EnvelopeProposer must be transient RoleNotActive, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn envelope_proposer_committee_executor_decodes_as_validator_executor() {
        // NOTE: This test documents a MessageId encoding constraint.
        // Due to MessageId encoding, Role::EnvelopeProposer is always decoded as
        // DutyExecutor::Validator (bytes 8-55), making Committee-executor EnvelopeProposer
        // impossible to construct via normal MessageId operations.
        //
        // This test verifies the encoding behavior: attempting to create a
        // Committee-executor EnvelopeProposer in MessageId results in a Validator-executor
        // being decoded, which then returns RoleNotActive (routing not wired).
        use fork::{Fork, ForkSchedule};
        use message_sender::testing::MockMessageSender;
        use ssv_types::{
            RSA_SIGNATURE_SIZE,
            consensus::{QbftMessage, QbftMessageType},
            message::{MsgType, SSVMessage, SignedSSVMessage},
        };
        use ssz::Encode;

        let setup = setup_test(1);

        // Create fork schedule with Boole active (latest SSV fork)
        let fork_schedule = ForkSchedule::new(Fork::Boole, DomainType::default(), "test");

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
            Arc::new(types::ChainSpec::mainnet()),
        )
        .expect("Manager creation should succeed");

        // Attempt to create EnvelopeProposer with Committee executor.
        // Due to MessageId encoding (Committee writes bytes 24-55, but EnvelopeProposer
        // decodes bytes 8-55 as Validator), this will be decoded as Validator executor.
        let msg_id = MessageId::new(
            &DomainType([0; 4]),
            Role::EnvelopeProposer,
            &DutyExecutor::Committee(CommitteeId([0; 32])),
        );

        let qbft_message = QbftMessage {
            qbft_message_type: QbftMessageType::Proposal,
            height: 100,
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

        // Act: Call receive_data
        let result = manager.receive_data(signed_msg, qbft_message);

        // Assert: Due to encoding, this is decoded as Validator executor,
        // so we get RoleNotActive (routing not wired), not InconsistentMessageId.
        assert!(
            matches!(result, Err(QbftError::RoleNotActive)),
            "EnvelopeProposer is always decoded as Validator, so expect RoleNotActive, got: {result:?}"
        );
    }
}

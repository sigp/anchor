use super::{
    CommitteeInstanceId, Completed, QbftDecidable, QbftError, QbftManager, WrappedQbftMessage,
};
use openssl::rsa::Rsa;
use processor::Senders;
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::consensus::{BeaconVote, QbftMessage, QbftMessageType};
use ssv_types::message::SignedSSVMessage;
use ssv_types::{Cluster, ClusterId, OperatorId};
use ssz::Decode;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::{Arc, RwLock, RwLockWriteGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use task_executor::{ShutdownReason, TaskExecutor};
use tokio::sync::mpsc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::error;
use types::{Hash256, Slot};

// Init tracing
static TRACING: LazyLock<()> = LazyLock::new(|| {
    let env_filter = tracing_subscriber::EnvFilter::new("debug");
    tracing_subscriber::fmt().with_env_filter(env_filter).init();
});

// Top level Testing Context to provide clean wrapper around testing framework
pub struct TestContext<D>
where
    D: QbftDecidable<ManualSlotClock>,
    D::Id: Send + Sync + Clone,
{
    pub tester: Arc<QbftTester<D>>,
    pub consensus_rx: UnboundedReceiver<ConsensusResult>,
}

impl<D> TestContext<D>
where
    D: QbftDecidable<ManualSlotClock>,
    D::Id: Send + Sync + Clone,
{
    // Create a new test context with default setup
    pub async fn new(
        clock: ManualSlotClock,
        executor: TaskExecutor,
        size: CommitteeSize,
        test_data: Vec<(D, D::Id)>,
    ) -> Self {
        let (mut tester, network_rx) = QbftTester::new(clock, executor, size);
        let result_rx = tester.start_instance(test_data).await;
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
        while let Some(result) = self.consensus_rx.recv().await {
            assert!(result.reached_consensus, "Consensus was not reached");
        }
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

impl CommitteeSize {
    // The number of fault nodes that the committee can tolerate
    fn get_f(&self) -> u64 {
        match self {
            CommitteeSize::Four => 1,
            CommitteeSize::Seven => 2,
            CommitteeSize::Ten => 3,
            CommitteeSize::Thirteen => 4,
        }
    }
}

/// The main test coordinator that manages multiple QBFT instances
pub struct QbftTester<D>
where
    D: QbftDecidable<ManualSlotClock>,
    D::Id: Send + Sync + Clone,
{
    // Senders to the processor
    senders: Senders,
    // Track mapping from operator id to the respective manager
    managers: HashMap<OperatorId, Arc<QbftManager<ManualSlotClock>>>,
    // The size of the committee
    size: CommitteeSize,
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

impl<D> QbftTester<D>
where
    D: QbftDecidable<ManualSlotClock> + 'static,
    D::Id: Send + Sync + Clone,
{
    /// Create a new QBFT tester instance
    pub fn new(
        slot_clock: ManualSlotClock,
        executor: TaskExecutor,
        size: CommitteeSize,
    ) -> (Self, mpsc::UnboundedReceiver<SignedSSVMessage>) {
        // Setup the processor
        let config = processor::Config { max_workers: 15 };
        let sender_queues = processor::spawn(config, executor);

        // Simulate the network sender and receiver. Qbft instances will send UnsignedSSVMessages
        // out on the network_tx and they will be received by the network_rx to be "signed" and then
        // broadcasted back into the instances
        let (network_tx, network_rx) = mpsc::unbounded_channel();

        // generate a random private key
        let pkey = Rsa::generate(2048).expect("Should not fail");

        // Construct and save a manager for each operator in the committee. By having access to all
        // the managers in the committee, we can direct messages to the proper place and
        // spawn multiple concurrent instances
        let mut managers = HashMap::new();
        let mut behavior = HashMap::new();
        for id in 1..=(size as u64) {
            let operator_id = OperatorId(id);
            let manager = QbftManager::new(
                sender_queues.clone(),
                operator_id,
                slot_clock.clone(),
                pkey.clone(),
                network_tx.clone(),
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
    // starting their own qbft instance when they must reach consensus with the rest of the committee
    pub async fn start_instance(
        &mut self,
        all_data: Vec<(D, D::Id)>,
    ) -> UnboundedReceiver<(Hash256, Result<Completed<D>, QbftError>)> {
        let (result_tx, result_rx) = mpsc::unbounded_channel();

        for (data, data_id) in all_data {
            let height = *data.instance_height(&data_id) as u64;
            self.identifiers.insert(height, data_id.clone());

            // Track the consensus results
            let min_for_consensus = self.size as u64 - self.size.get_f();
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
            for manager in self.managers.values() {
                let manager_clone = manager.clone();
                let cluster = self.cluster.clone();
                let data_clone = data.clone();
                let id_clone = data_id.clone();
                let tx_clone = result_tx.clone();

                // decide the instance
                let _ = self.senders.permitless.send_async(
                    async move {
                        // Operator is online, start the instance
                        let result = manager_clone
                            .decide_instance(id_clone, data_clone.clone(), &cluster)
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
                Some(signed) = network_rx.recv() => {
                    // We have a signed ssv message. The next step is to then broadcast this onto
                    // the network. Here, we will just mock this now being recieved by all of the
                    // other instances
                    let wrapped = self.signed_to_wrapped(signed);

                    self.process_network_message(wrapped);
                },

                Some((hash, completion)) = result_rx.recv() => {
                    self.handle_completion(hash, completion);
                    if self.finished() {
                        for res in self.results.read().unwrap().values().cloned(){
                            let _ = consensus_tx.send(res);
                        }
                        break;
                    }
                }
            }
        }
        // drop so the consensus receiver gets a close notifcation
        drop(consensus_tx);
    }

    fn signed_to_wrapped(&self, signed: SignedSSVMessage) -> WrappedQbftMessage {
        let deser_qbft = QbftMessage::from_ssz_bytes(signed.ssv_message().data())
            .expect("We have a valid qbft message");
        WrappedQbftMessage {
            signed_message: signed,
            qbft_message: deser_qbft,
        }
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
        for running in self.num_running.read().unwrap().values() {
            finished &= *running <= self.size.get_f();
        }
        finished
    }

    // Process and send a network message to the correct instance
    fn process_network_message(&self, mut wrapped_msg: WrappedQbftMessage) {
        let sender_operator_id = wrapped_msg
            .signed_message
            .operator_ids()
            .first()
            .expect("One signer");
        let sender_operator_id = OperatorId::from(*sender_operator_id);

        // Now we have a message ready to be sent back into the instance. Get the id
        // corresponding to the message.
        let data_id = self
            .identifiers
            .get(&wrapped_msg.qbft_message.height)
            .expect("Value exists");

        // Check the sender behavior
        let sender_behavior = self.get_behavior(&sender_operator_id);
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
                let _ = manager.receive_data::<D>(data_id.clone(), message.clone());
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
}

#[cfg(test)]
mod manager_tests {
    use super::*;

    // Provides test setup
    struct Setup {
        executor: TaskExecutor,
        _signal: async_channel::Sender<()>,
        _shutdown: futures::channel::mpsc::Sender<ShutdownReason>,
        clock: ManualSlotClock,
        all_data: Vec<(BeaconVote, CommitteeInstanceId)>,
    }

    // Generate unique test data
    fn generate_test_data(id: usize) -> (BeaconVote, CommitteeInstanceId) {
        // setup mock data
        let id = CommitteeInstanceId {
            committee: ClusterId([0; 32]),
            instance_height: id.into(),
        };

        let data = BeaconVote {
            block_root: Hash256::random(),
            source: types::Checkpoint::default(),
            target: types::Checkpoint::default(),
        };

        (data, id)
    }

    // Setup env for the test
    fn setup_test(num_instances: usize) -> Setup {
        *TRACING;

        // setup the executor
        let handle = tokio::runtime::Handle::current();
        let (signal, exit) = async_channel::bounded(1);
        let (shutdown, _) = futures::channel::mpsc::channel(1);
        let executor = TaskExecutor::new(handle, exit, shutdown.clone());

        // setup the slot clock
        let slot_duration = Duration::from_secs(12);
        let genesis_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let clock = ManualSlotClock::new(
            Slot::new(0),
            Duration::from_secs(genesis_time),
            slot_duration,
        );

        let mut all_data = vec![];
        for id in 1..num_instances + 1 {
            all_data.push(generate_test_data(id))
        }

        Setup {
            executor,
            _signal: signal,
            _shutdown: shutdown,
            clock,
            all_data,
        }
    }

    #[tokio::test]
    // Test running a single instance and confirm that it reaches consensus
    async fn test_basic_run() {
        let setup = setup_test(1);
        let mut context = TestContext::<BeaconVote>::new(
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
        let mut context = TestContext::<BeaconVote>::new(
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
        let mut context = TestContext::<BeaconVote>::new(
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
            let mut context = TestContext::<BeaconVote>::new(
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
        let mut context = TestContext::<BeaconVote>::new(
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
        let mut context = TestContext::<BeaconVote>::new(
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
    // Test commit message supression for an operator
    async fn test_commit_suppression() {
        let setup = setup_test(1);
        let mut context = TestContext::<BeaconVote>::new(
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
        let mut context = TestContext::<BeaconVote>::new(
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
        let mut context = TestContext::<BeaconVote>::new(
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
        let mut context = TestContext::<BeaconVote>::new(
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
}

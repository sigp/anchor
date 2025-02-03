use super::{
    CommitteeInstanceId, Completed, QbftData, QbftDecidable, QbftError, QbftManager,
    WrappedQbftMessage,
};
use processor::Senders;
use qbft::Message;
use slot_clock::{SlotClock, SystemTimeSlotClock};
use ssv_types::consensus::{BeaconVote, QbftMessage};
use ssv_types::message::SignedSSVMessage;
use ssv_types::{Cluster, ClusterId, OperatorId};
use ssz::Decode;
use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockWriteGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use task_executor::{ShutdownReason, TaskExecutor};
use tokio::sync::mpsc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use types::{Hash256, Slot};

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
pub struct QbftTester<T, D>
where
    T: SlotClock + 'static,
    D: QbftDecidable<T>,
{
    // Senders to the processor
    senders: Senders,
    // Track mapping from operator id to the respective manager
    managers: HashMap<OperatorId, Arc<QbftManager<T>>>,
    // Used to recieve messages from the qbft instances
    network_rx: UnboundedReceiver<Message>,
    // Channels for sending and receiving results once instances have been decided
    result_rx: UnboundedReceiver<(Hash256, Result<Completed<D>, QbftError>)>,
    result_tx: UnboundedSender<(Hash256, Result<Completed<D>, QbftError>)>,
    // The size of the committee
    size: CommitteeSize,
    // Mapping of the data hash to the data identifier. This is to send data to the proper instance
    identifiers: HashMap<Hash256, D::Id>,
    // Mapping from data to the results of the consensus
    results: HashMap<Hash256, ConsensusResult>,
    // The number of individual qbft instances that are running at any given moment
    num_running: HashMap<Hash256, u64>,
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
    Delayed(Duration),
}

// Descirbes the behavior of an operator
#[derive(Clone, Debug, Default, Copy)]
pub struct OperatorBehavior {
    // Id of the operator
    pub operator_id: OperatorId,
    // If this operator is online or not
    pub status: OperationalStatus,
}

impl OperatorBehavior {
    pub fn new(operator_id: OperatorId) -> Self {
        Self {
            operator_id,
            status: OperationalStatus::Online,
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
}

impl<T, D> QbftTester<T, D>
where
    T: SlotClock + 'static,
    D: QbftDecidable<T> + 'static,
{
    /// Create a new QBFT tester instance
    pub fn new(slot_clock: T, executor: TaskExecutor, size: CommitteeSize) -> Self {
        // Setup the processor
        let config = processor::Config { max_workers: 15 };
        let sender_queues = processor::spawn(config, executor);

        // Simulate the network sender and receiver. Qbft instances will send UnsignedSSVMessages
        // out on the network_tx and they will be recieved by the network_rx to be "signed" and then
        // multicast broadcasted back into the instances for simulation
        let (network_tx, network_rx) = mpsc::unbounded_channel();

        // Send and recieve the result of the instance
        let (result_tx, result_rx) = mpsc::unbounded_channel();

        // Construct and save a manager for each operator in the committee. By having access to all
        // the managers in the committee, we can properly direct messages to the proper place and
        // spawn multiple concurrent instances
        let mut managers = HashMap::new();
        let mut behavior = HashMap::new();
        for id in 1..=(size as u64) {
            let operator_id = OperatorId(id);
            let manager = QbftManager::new(
                sender_queues.clone(),
                operator_id,
                slot_clock.clone(),
                network_tx.clone(),
            )
            .expect("Creation should not fail");

            managers.insert(operator_id, manager);

            behavior.insert(
                operator_id,
                Arc::new(RwLock::new(OperatorBehavior::new(operator_id))),
            );
        }

        // Dummy cluster
        let cluster = Cluster {
            cluster_id: ClusterId([0; 32]),
            owner: Default::default(),
            fee_recipient: Default::default(),
            faulty: size.get_f(),
            liquidated: false,
            cluster_members: (1..=(size as u64)).map(OperatorId).collect(),
        };

        Self {
            senders: sender_queues,
            identifiers: HashMap::new(),
            managers,
            result_tx,
            result_rx,
            network_rx,
            size,
            results: HashMap::new(),
            num_running: HashMap::new(),
            cluster,
            behavior,
        }
    }

    // Start a new full test instance for the provided configuration. This will start a new qbft
    // instance for each operator in the committee. This simulates distributed instances each
    // starting their own qbft instance when they must reach consensus with the rest of the committee
    pub async fn start_instance(&mut self, all_data: Vec<(D, D::Id)>) -> Result<(), QbftError> {
        for (data, data_id) in all_data {
            // Record mapping of hash => id. This allows us to identify the instances as we only
            // have access to data roots in the messages
            self.identifiers.insert(data.hash(), data_id.clone());

            // Track the consensus results
            let result = ConsensusResult::default();
            self.results.insert(data.hash(), result);

            // Record that we have self.size instances running
            self.num_running.insert(data.hash(), self.size as u64);

            // Go through all of the managers. Spawn a new instance for the data and record it
            for manager in self.managers.values() {
                let manager_clone = manager.clone();
                let cluster = self.cluster.clone();
                let data_clone = data.clone();
                let id_clone = data_id.clone();
                let tx_clone = self.result_tx.clone();

                // decide the instance
                let _ = self.senders.permitless.send_async(
                    async move {
                        // Operator is online, start the instance
                        let result = manager_clone
                            .decide_instance(id_clone, data_clone.clone(), &cluster)
                            .await;
                        let _ = tx_clone.send((data_clone.hash(), result));
                    },
                    "qbft_instance starting",
                );
            }
        }

        Ok(())
    }

    // Get a write lock to the behavior so that we can modify it while the instance is running
    fn modify_behavior(&self, id: OperatorId) -> RwLockWriteGuard<'_, OperatorBehavior> {
        self.behavior
            .get(&id)
            .expect("value exist")
            .write()
            .expect("value exist")
    }

    // Get the behavior for the operator
    fn get_behavior(&self, id: OperatorId) -> Arc<RwLock<OperatorBehavior>> {
        self.behavior.get(&id).expect("Exists").clone()
    }

    // When all the instances are spawned, handle all outgoing messages
    async fn run_until_complete(&mut self) -> Vec<ConsensusResult> {
        loop {
            tokio::select! {
                // Try to recieve a network message
                Some(qbft_message) = async { self.network_rx.try_recv().ok() } => {
                    self.process_network_message(qbft_message);
                },
                // Try to see if a instance has completed
                Some((hash, completion)) = async { self.result_rx.try_recv().ok() } => {
                    self.handle_completion(hash, completion);
                    if self.finished() {
                        return self.results.values().cloned().collect();
                    }
                }
                // Have to yield here. try_recv is greedy and will starve the runtime
                else => {
                    tokio::task::yield_now().await;
                }
            }
        }
    }

    // Once an instance has completed, we want to record what happened
    fn handle_completion(&mut self, hash: Hash256, msg: Result<Completed<D>, QbftError>) {
        // Decrement the amount of instances running for this data
        let num = self.num_running.get_mut(&hash).expect("this exists");
        *num -= 1;

        match msg {
            Ok(completed) => match completed {
                Completed::Success(_) => {
                    let results = self.results.get_mut(&hash).expect("This exists");
                    results.successful += 1;

                    if results.successful >= results.min_for_consensus {
                        results.reached_consensus = true;
                    }
                }
                Completed::TimedOut => todo!(),
            },
            Err(e) => {
                println!("{:?}", e)
            }
        }
    }

    // Check if all of the instances have finished running
    fn finished(&self) -> bool {
        let mut finished = true;
        for running in self.num_running.values() {
            finished &= *running <= self.size.get_f();
        }
        finished
    }

    // Process and send a network message to the correct instance
    fn process_network_message(&self, msg: Message) {
        let (sender_operator_id, unsigned_msg) = match msg {
            Message::Propose(id, msg) => (id, msg),
            Message::Prepare(id, msg) => (id, msg),
            Message::Commit(id, msg) => (id, msg),
            Message::RoundChange(id, msg) => (id, msg),
        };
        // First decode the QBFT message to get the instance identifier
        let qbft_msg = match QbftMessage::from_ssz_bytes(unsigned_msg.ssv_message.data()) {
            Ok(msg) => msg,
            Err(_) => todo!(),
        };

        // Create wrapped message
        let signed_msg = SignedSSVMessage::new(
            vec![vec![0; 96]], // Test signature
            vec![*sender_operator_id],
            unsigned_msg.ssv_message.clone(),
            unsigned_msg.full_data,
        )
        .expect("Failed to create signed message");

        let wrapped_msg = WrappedQbftMessage {
            signed_message: signed_msg,
            qbft_message: qbft_msg.clone(),
        };

        // Now we have a message ready to be sent back into the instance. Get the id
        // corresponding to the message. and then all the managers that are running instances
        // for this data
        let data_id = self.identifiers.get(&qbft_msg.root).expect("Value exists");

        // Check the sender behavior
        let sender_behavior = self.get_behavior(sender_operator_id);
        let sender_read = sender_behavior.read().expect("Exists");
        if sender_read.is_offline() {
            return;
        }

        // for each operator, send the message to the instance for the data
        for id in 1..=(self.size as u64) {
            let operator_id = OperatorId::from(id);
            let manager = self.managers.get(&operator_id).unwrap().clone();

            // Check the reciever behavior
            let receiver_behavior = self.get_behavior(operator_id);
            let receiver_read = receiver_behavior.read().expect("Exists");
            if receiver_read.is_offline() {
                continue;
            }

            let _ = manager.receive_data::<D>(data_id.clone(), wrapped_msg.clone());
        }
    }
}

#[derive(Clone, Default)]
pub struct ConsensusResult {
    reached_consensus: bool,
    min_for_consensus: u64,
    successful: u64,
}

#[cfg(test)]
mod manager_tests {
    use super::*;
    use rand::random;

    // Provides test setup
    struct Setup {
        executor: TaskExecutor,
        _signal: async_channel::Sender<()>,
        _shutdown: futures::channel::mpsc::Sender<ShutdownReason>,
        clock: SystemTimeSlotClock,
    }

    // Generate unique test data
    fn generate_test_data() -> (BeaconVote, CommitteeInstanceId) {
        // setup mock data
        let rand_id: [u8; 32] = [(); 32].map(|_| random());
        let id = CommitteeInstanceId {
            committee: ClusterId(rand_id),
            instance_height: 10.into(),
        };

        let data = BeaconVote {
            block_root: Hash256::random(),
            source: types::Checkpoint::default(),
            target: types::Checkpoint::default(),
        };

        (data, id)
    }

    // Setup env for the test
    fn setup_test() -> Setup {
        let env_filter = tracing_subscriber::EnvFilter::new("debug");
        tracing_subscriber::fmt().with_env_filter(env_filter).init();

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

        let clock = SystemTimeSlotClock::new(
            Slot::new(0),
            Duration::from_secs(genesis_time),
            slot_duration,
        );

        Setup {
            executor,
            _signal: signal,
            _shutdown: shutdown,
            clock,
        }
    }

    #[tokio::test]
    // Test running a single instance and confirm that it reaches consensus
    async fn test_basic_run() {
        // Standard setup
        let setup = setup_test();

        // Setup the tester
        let mut tester: QbftTester<SystemTimeSlotClock, BeaconVote> =
            QbftTester::new(setup.clock, setup.executor, CommitteeSize::Thirteen);

        let data = vec![generate_test_data()];
        tester
            .start_instance(data)
            .await
            .expect("Should start instance");

        // Wait for it to run and confirm all reached consensus
        // Confirm that we reached consensus
        for res in tester.run_until_complete().await {
            assert!(res.reached_consensus);
        }
    }

    #[tokio::test]
    // Test one offline operator
    async fn test_fault_operator() {
        // Standard setup
        let setup = setup_test();

        // Setup the tester
        let mut tester: QbftTester<SystemTimeSlotClock, BeaconVote> =
            QbftTester::new(setup.clock, setup.executor, CommitteeSize::Four);

        // Take operator 1 offline
        tester.modify_behavior(OperatorId::from(1)).set_offline();

        tester
            .start_instance(vec![generate_test_data()])
            .await
            .expect("should start instance");

        for res in tester.run_until_complete().await {
            assert!(res.reached_consensus);
        }
    }

    #[tokio::test]
    // Test running concurrent instances and confirm that they reach consensus
    async fn test_concurrent_runs() {
        // Standard setup
        let setup = setup_test();

        // Setup the tester
        let mut tester: QbftTester<SystemTimeSlotClock, BeaconVote> =
            QbftTester::new(setup.clock, setup.executor, CommitteeSize::Four);

        let data = vec![generate_test_data(), generate_test_data()];
        tester
            .start_instance(data)
            .await
            .expect("Should start instance");

        // Wait for it to run and confirm all reached consensus
        // Confirm that we reached consensus
        for res in tester.run_until_complete().await {
            assert!(res.reached_consensus);
        }
    }

    #[tokio::test]
    // Start with > f fault and then recover them. This should reach consensus
    async fn test_recovery() {
        // Standard setup
        let setup = setup_test();

        // Setup the tester
        let mut tester: QbftTester<SystemTimeSlotClock, BeaconVote> =
            QbftTester::new(setup.clock, setup.executor, CommitteeSize::Four);

        // Take operator 1 & 2 offline
        tester.modify_behavior(OperatorId::from(1)).set_offline();
        tester.modify_behavior(OperatorId::from(2)).set_offline();

        tester
            .start_instance(vec![generate_test_data()])
            .await
            .expect("should start instance");

        // sleep and then take them back online
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        tester.modify_behavior(OperatorId::from(1)).set_online();
        tester.modify_behavior(OperatorId::from(2)).set_online();

        for res in tester.run_until_complete().await {
            assert!(res.reached_consensus);
        }
    }
}

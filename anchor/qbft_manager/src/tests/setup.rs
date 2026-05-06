use super::*;

// Provides test setup
pub(super) struct Setup {
    pub(super) executor: TaskExecutor,
    pub(super) _signal: async_channel::Sender<()>,
    pub(super) _shutdown: futures::channel::mpsc::Sender<ShutdownReason>,
    pub(super) clock: ManualSlotClock,
    pub(super) all_data: Vec<(BeaconVote, CommitteeInstanceId)>,
}

// Generate unique test data
pub(crate) fn generate_test_data(id: usize) -> (BeaconVote, CommitteeInstanceId) {
    // setup mock data
    let id = CommitteeInstanceId {
        committee: CommitteeId([0; 32]),
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
pub(super) fn setup_test(num_instances: usize) -> Setup {
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

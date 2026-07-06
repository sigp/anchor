use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use database::{NetworkState, UniqueIndex};
use fork::{Fork, ForkConfig, ForkLifecycle};
use slot_clock::SlotClock;
use ssv_types::OperatorId;
use tokio::{sync::watch, time::sleep};
use tracing::{debug, error, warn};
use types::EthSpec;

use crate::{
    SUBNET_COUNT, SubnetId, SubnetServiceError, TopicEvent, service::SubnetService,
    topic::create_topic,
};

pub(crate) struct ServiceState {
    pub(crate) forks: HashMap<Fork, ForkSubscriptions>,
    pub(crate) fork_to_score: Fork,
}

impl ServiceState {
    /// Make sure we only track the passed forks. Returns any forks that were removed.
    fn set_subscribed_forks<const N: usize>(
        &mut self,
        forks: [ForkConfig; N],
    ) -> Vec<ForkSubscriptions> {
        // Remove forks that we should no longer be subscribed to.
        let removed = self
            .forks
            .extract_if(|fork, _| !forks.iter().any(|f| f.fork == *fork))
            .map(|(_, v)| v)
            .collect();
        // Ensure we track subscriptions for all forks that we should be subscribed to.
        for fork in forks {
            self.forks.entry(fork.fork).or_insert(ForkSubscriptions {
                config: fork.clone(),
                currently_subscribed: HashSet::new(),
            });
        }
        removed
    }
}

pub(crate) struct ForkSubscriptions {
    pub(crate) config: ForkConfig,
    pub(crate) currently_subscribed: HashSet<SubnetId>,
}

enum SubnetGenerator {
    All,
    Committees(HashSet<Vec<OperatorId>>),
}

impl SubnetGenerator {
    fn committees_from_network_state(state: &NetworkState) -> Self {
        let all_committee_members = state
            .get_own_clusters()
            .iter()
            .flat_map(|cluster| state.clusters().get_by(cluster))
            // Liquidated clusters perform no duties, so their subnets need no subscription.
            .filter(|cluster| !cluster.liquidated)
            .map(|cluster| cluster.cluster_members.iter().copied().collect::<Vec<_>>())
            .collect();
        SubnetGenerator::Committees(all_committee_members)
    }

    fn generate_subnets(&self, fork: Fork) -> Result<HashSet<SubnetId>, SubnetServiceError> {
        let subnets = match self {
            SubnetGenerator::All => (0..SUBNET_COUNT)
                .map(|subnet_id| SubnetId::new(subnet_id as u64))
                .collect(),
            SubnetGenerator::Committees(committees) => committees
                .iter()
                .map(|committee| {
                    SubnetId::from_operators_for_fork(committee, fork)
                        .map_err(SubnetServiceError::SubnetCalculation)
                })
                .collect::<Result<_, _>>()?,
        };
        Ok(subnets)
    }
}

impl<S: SlotClock> SubnetService<S> {
    /// Main background task that manages subnet subscriptions and scoring updates.
    ///
    /// This method takes `Arc<Self>` to allow the service to be shared while running.
    pub async fn run<E: EthSpec>(
        self: Arc<Self>,
        mut lifecycle_rx: watch::Receiver<ForkLifecycle>,
    ) {
        let mut db = self.db.clone();
        let mut service_state = self.initial_service_state::<E>(&mut lifecycle_rx).await;

        loop {
            let delay = calculate_duration_to_next_epoch::<E>(&*self.slot_clock);
            tokio::select! {
                _ = db.changed(), if !self.subscribe_all_subnets => {
                    self.handle_subnet_changes::<E>(&mut service_state).await;
                }
                _ = sleep(delay), if !self.disable_gossipsub_topic_scoring => {
                    self.send_scoring_rate_updates::<E>(&service_state).await;
                }
                Ok(()) = lifecycle_rx.changed() => {
                    let new_lifecycle = lifecycle_rx.borrow_and_update().clone();
                    self.on_lifecycle_transition(new_lifecycle, &mut service_state).await;
                    self.handle_subnet_changes::<E>(&mut service_state).await;
                }
            }
        }
    }

    /// Handle a lifecycle state transition by updating the service's fork subscriptions.
    ///
    /// Compares the previous and new lifecycle states and performs the corresponding
    /// subscription management:
    /// - Normal → WarmUp: insert upcoming fork (subscribe to new topics)
    /// - WarmUp/Normal → GracePeriod: update fork_to_score, insert current fork
    /// - GracePeriod → Normal: remove & unsubscribe previous fork
    /// - GracePeriod → WarmUp: remove previous + insert upcoming (overlapping transition)
    async fn on_lifecycle_transition(&self, new: ForkLifecycle, service_state: &mut ServiceState) {
        service_state.fork_to_score = new.current_fork_config().fork;
        let removed = match new {
            ForkLifecycle::Normal { current } => service_state.set_subscribed_forks([current]),
            ForkLifecycle::WarmUp { current, upcoming } => {
                service_state.set_subscribed_forks([current, upcoming])
            }
            ForkLifecycle::GracePeriod { current, previous } => {
                service_state.set_subscribed_forks([current, previous])
            }
        };
        for removed in removed {
            if let Err(err) = self
                .send_unsubscribes(
                    &removed.config,
                    removed.currently_subscribed, // uses the set before dropping it
                )
                .await
            {
                error!("Failed to unsubscribe from fork: {:?}", err);
            }
        }
    }

    async fn initial_service_state<E: EthSpec>(
        &self,
        lifecycle_rx: &mut watch::Receiver<ForkLifecycle>,
    ) -> ServiceState {
        let mut service_state = ServiceState {
            forks: HashMap::new(),
            // This will be updated by the `on_lifecycle_transition` call below.
            fork_to_score: Fork::Alan,
        };

        let initial_lifecycle = lifecycle_rx.borrow_and_update().clone();
        self.on_lifecycle_transition(initial_lifecycle, &mut service_state)
            .await;

        self.handle_subnet_changes::<E>(&mut service_state).await;

        service_state
    }

    /// Compare current and previous subnets, emitting subscribe/unsubscribe events.
    ///
    /// High-level flow:
    /// 1) Build the current subnet set + topic prefix for the current slot.
    /// 2) Diff against the last recorded subnets to emit unsubscribes/subscribes.
    /// 3) If a fork transition is active, manage transition topics in parallel.
    ///
    /// The transition path is careful to avoid duplicate subscriptions when the
    /// transition prefix matches the current prefix.
    async fn handle_subnet_changes<E: EthSpec>(&self, service_state: &mut ServiceState) {
        let subnet_source = if self.subscribe_all_subnets {
            SubnetGenerator::All
        } else {
            let state = self.db.borrow();
            SubnetGenerator::committees_from_network_state(&state)
        };

        for fork in service_state.forks.values_mut() {
            let score = fork.config.fork == service_state.fork_to_score;
            if let Err(err) = self
                .handle_subnet_changes_for_fork::<E>(&subnet_source, fork, score)
                .await
            {
                error!(fork=?fork.config.fork, ?err, "Failed to handle subnet changes for fork")
            }
        }
    }

    async fn handle_subnet_changes_for_fork<E: EthSpec>(
        &self,
        subnet_generator: &SubnetGenerator,
        fork: &mut ForkSubscriptions,
        score: bool,
    ) -> Result<(), SubnetServiceError> {
        let target_subnets = subnet_generator.generate_subnets(fork.config.fork)?;

        let to_leave = fork
            .currently_subscribed
            .difference(&target_subnets)
            .copied();
        let to_join = target_subnets
            .difference(&fork.currently_subscribed)
            .copied();

        self.send_unsubscribes(&fork.config, to_leave).await?;
        self.send_subscribes::<E>(&fork.config, to_join, score)
            .await?;

        fork.currently_subscribed = target_subnets;
        Ok(())
    }

    /// Emit unsubscribe events for the given prefix/subnets.
    async fn send_unsubscribes<I>(
        &self,
        fork_config: &ForkConfig,
        subnets: I,
    ) -> Result<(), SubnetServiceError>
    where
        I: IntoIterator<Item = SubnetId>,
    {
        let prefix = fork_config
            .fork
            .topic_prefix(self.router().fork_schedule().network_name());
        for subnet in subnets {
            let topic = create_topic(&prefix, subnet);
            debug!(%topic, "send unsubscribe");
            if self
                .tx
                .send(TopicEvent::Unsubscribe { topic, subnet })
                .await
                .is_err()
            {
                warn!("Network no longer listening for topic events");
                return Err(SubnetServiceError::SendFailed);
            }
        }
        Ok(())
    }

    /// Emit subscribe events for current subnets. If `send_message_rate` is true, also emit
    /// scoring rate events, unless they are disabled.
    async fn send_subscribes<E: EthSpec>(
        &self,
        fork_config: &ForkConfig,
        subnets: impl IntoIterator<Item = SubnetId>,
        send_message_rate: bool,
    ) -> Result<(), SubnetServiceError> {
        let prefix = fork_config
            .fork
            .topic_prefix(self.router().fork_schedule().network_name());
        for subnet in subnets {
            let topic = create_topic(&prefix, subnet);
            debug!(%topic, "send subscribe");
            let message_rate = send_message_rate
                .then(|| {
                    let state = self.db.borrow();
                    self.subnet_message_rate::<E>(&subnet, fork_config, &state)
                })
                .flatten();

            if self
                .tx
                .send(TopicEvent::Subscribe {
                    topic,
                    subnet,
                    message_rate,
                })
                .await
                .is_err()
            {
                warn!("Network no longer listening for topic events");
                return Err(SubnetServiceError::SendFailed);
            }
        }
        Ok(())
    }
}

/// Calculate duration until the next epoch boundary.
fn calculate_duration_to_next_epoch<E: EthSpec>(slot_clock: &impl SlotClock) -> Duration {
    if let Some(duration_to_next_epoch) = slot_clock.duration_to_next_epoch(E::slots_per_epoch()) {
        duration_to_next_epoch
    } else {
        // Fallback: if we can't get current slot, use a conservative short interval
        let slot_duration = slot_clock.slot_duration();
        warn!("Could not get current slot for epoch delay calculation, using fallback timing");
        slot_duration * 3 // Wait 3 slots before next check
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{Arc, OnceLock},
        time::Duration,
    };

    use database::{
        NetworkDatabase, PendingStateUpdates,
        test_utils::{DEFAULT_NUM_OPERATORS, commit_and_publish, generators},
    };
    use fork::{ALAN_TOPIC_PREFIX, Fork, ForkConfig, ForkLifecycle, ForkSchedule};
    use rusqlite::Transaction;
    use slot_clock::{ManualSlotClock, SlotClock};
    use ssv_types::{Cluster, ClusterId, Operator, OperatorId, domain_type::DomainType};
    use task_executor::test_utils::TestRuntime;
    use tempfile::TempDir;
    use tokio::{
        sync::{mpsc, watch},
        time::timeout,
    };
    use types::{
        ChainSpec, Epoch, MinimalEthSpec, Slot, test_utils::generate_deterministic_keypair,
    };

    use crate::{SUBNET_COUNT, SubnetId, TopicEvent, start_subnet_service};

    const TEST_NETWORK: &str = "test";
    const ALAN_DOMAIN: DomainType = DomainType([0, 0, 0, 1]);
    const BOOLE_DOMAIN: DomainType = DomainType([0, 0, 0, 2]);
    const BOOLE_FORK_EPOCH: u64 = 100;
    const EVENT_TIMEOUT: Duration = Duration::from_secs(2);
    const NO_EXTRA_EVENTS_TIMEOUT: Duration = Duration::from_millis(50);
    /// Operator id the test database impersonates; clusters whose shares include it become own
    /// clusters.
    const OWN_OPERATOR_ID: OperatorId = OperatorId(1);

    /// Test harness that spins up a live `SubnetService` and captures emitted topic events.
    ///
    /// The harness uses real watch/mpsc channels so tests exercise the `run()` loop end-to-end
    /// for lifecycle transitions.
    struct TestHarness {
        db: NetworkDatabase,
        _runtime: TestRuntime,
        service: Arc<crate::SubnetService<ManualSlotClock>>,
        lifecycle_tx: watch::Sender<ForkLifecycle>,
        topic_event_rx: mpsc::Receiver<TopicEvent>,
        alan_config: ForkConfig,
        boole_config: ForkConfig,
        // Keep this last so it drops after db/runtime/service and can clean up directory safely.
        _temp_dir: TempDir,
    }

    impl TestHarness {
        fn new_normal_on_alan() -> Self {
            Self::new_with_initial_lifecycle(normal_on_alan)
        }

        fn new_warmup_alan_to_boole() -> Self {
            Self::new_with_initial_lifecycle(warmup_from_alan_to_boole)
        }

        fn new_grace_period_current_boole_previous_alan() -> Self {
            Self::new_with_initial_lifecycle(grace_period_current_boole_previous_alan)
        }

        /// Committee-driven subscriptions: subnets come from the own clusters in the database,
        /// so cluster changes (insert/liquidate/reactivate) are observable as topic events.
        fn new_normal_on_alan_committee_subscriptions() -> Self {
            Self::new_with_flags(normal_on_alan, false, true)
        }

        /// Committee-driven subscriptions with gossipsub topic scoring enabled, because
        /// `subnet_message_rate` returns `None` when scoring is disabled.
        fn new_normal_on_alan_with_scoring() -> Self {
            Self::new_with_flags(normal_on_alan, false, false)
        }

        /// Build a service with an initial lifecycle and ready-to-assert event receiver.
        fn new_with_initial_lifecycle(
            initial_lifecycle: impl FnOnce(&ForkConfig, &ForkConfig) -> ForkLifecycle,
        ) -> Self {
            Self::new_with_flags(
                initial_lifecycle,
                // Keep lifecycle tests deterministic: always subscribe to all subnets so each
                // lifecycle phase emits a stable `SUBNET_COUNT` per tracked fork, independent of
                // DB committee fixture contents.
                true,
                // Lifecycle tests only validate subscribe/unsubscribe transitions, so disable
                // periodic scoring updates to avoid unrelated topic-event traffic.
                true,
            )
        }

        /// Build a service with an initial lifecycle and explicit subscription/scoring flags.
        fn new_with_flags(
            initial_lifecycle: impl FnOnce(&ForkConfig, &ForkConfig) -> ForkLifecycle,
            subscribe_all_subnets: bool,
            disable_gossipsub_topic_scoring: bool,
        ) -> Self {
            let temp_dir = TempDir::new().expect("should create temp directory for test database");
            let db_path = temp_dir.path().join("subnet_service_lifecycle.db");
            let db = NetworkDatabase::new_as_impostor(&db_path, &OWN_OPERATOR_ID, TEST_NETWORK)
                .expect("should build test database");

            let (fork_schedule, alan_config, boole_config) = test_fork_schedule();
            let (lifecycle_tx, lifecycle_rx) =
                watch::channel(initial_lifecycle(&alan_config, &boole_config));

            let slot_clock = ManualSlotClock::new(
                Slot::new(0),
                Duration::from_secs(0),
                Duration::from_secs(12),
            );

            let runtime = TestRuntime::default();
            let (service, topic_event_rx) = start_subnet_service::<_, MinimalEthSpec>(
                db.watch(),
                subscribe_all_subnets,
                disable_gossipsub_topic_scoring,
                &runtime.task_executor,
                slot_clock,
                Arc::new(ChainSpec::minimal()),
                fork_schedule,
                lifecycle_rx,
            );

            Self {
                db,
                _runtime: runtime,
                service,
                lifecycle_tx,
                topic_event_rx,
                alan_config,
                boole_config,
                _temp_dir: temp_dir,
            }
        }

        /// Push a lifecycle transition into the service's watch channel.
        fn send_lifecycle(
            &self,
            lifecycle: impl FnOnce(&ForkConfig, &ForkConfig) -> ForkLifecycle,
        ) {
            self.lifecycle_tx
                .send(lifecycle(&self.alan_config, &self.boole_config))
                .expect("subnet service should still listen for lifecycle transitions");
        }

        fn transition_to_warmup_alan_to_boole(&self) {
            self.send_lifecycle(warmup_from_alan_to_boole);
        }

        fn transition_to_normal_on_boole(&self) {
            self.send_lifecycle(normal_on_boole);
        }

        fn transition_to_grace_period_current_boole_previous_alan(&self) {
            self.send_lifecycle(grace_period_current_boole_previous_alan);
        }

        /// Receive exactly `count` transition events, failing fast on timeout/channel close.
        async fn recv_transition_events(&mut self, count: usize) -> Vec<TopicEvent> {
            let mut events = Vec::with_capacity(count);
            for _ in 0..count {
                events.push(
                    timeout(EVENT_TIMEOUT, self.topic_event_rx.recv())
                        .await
                        .expect("timed out waiting for topic event")
                        .expect("topic event channel closed unexpectedly"),
                );
            }
            events
        }

        /// Consume startup subscriptions emitted immediately when the service boots.
        async fn consume_startup_events(&mut self, count: usize) {
            let startup_events = self.recv_transition_events(count).await;
            assert_all_events_match(&startup_events, "subscribe event", is_subscribe_event);
        }

        async fn assert_no_additional_events(&mut self) {
            match timeout(NO_EXTRA_EVENTS_TIMEOUT, self.topic_event_rx.recv()).await {
                Ok(Some(event)) => panic!(
                    "unexpected extra topic event after assertion boundary: {}",
                    describe_topic_event(&event)
                ),
                Ok(None) => panic!("topic event channel closed unexpectedly"),
                Err(_) => {}
            }
        }

        /// Run database mutations inside one transaction, then commit and publish the queued
        /// state updates to the running service.
        fn mutate_db(
            &self,
            mutate: impl FnOnce(&NetworkDatabase, &Transaction<'_>, &mut PendingStateUpdates),
        ) {
            let mut conn = self
                .db
                .connection()
                .expect("should get database connection");
            let tx = conn.transaction().expect("should begin transaction");
            let mut pending = PendingStateUpdates::default();
            mutate(&self.db, &tx, &mut pending);
            commit_and_publish(&self.db, tx, pending);
        }

        /// Insert network operators and publish the state update to the running service.
        fn insert_operators(&self, operators: &[Operator]) {
            self.mutate_db(|db, tx, pending| {
                for operator in operators {
                    db.insert_operator_tx(operator, tx, pending)
                        .expect("should insert operator");
                }
            });
        }

        /// Insert a cluster of `operators` with one validator and publish the state update.
        ///
        /// The shares cover every operator, so the share for `OWN_OPERATOR_ID` makes this an
        /// own cluster. `pubkey_seed` must be unique per inserted validator because the pubkey
        /// generator in `database::test_utils` is deterministically seeded and reusing a pubkey
        /// collides on the validators table.
        fn insert_own_cluster(&self, operators: &[Operator], pubkey_seed: usize) -> Cluster {
            let cluster = generators::cluster::with_operators(operators);
            let mut validator = generators::validator::random_metadata(cluster.cluster_id);
            validator.public_key = generate_deterministic_keypair(pubkey_seed).pk.compress();
            let shares = operators
                .iter()
                .map(|operator| {
                    generators::share::random(
                        cluster.cluster_id,
                        operator.id,
                        &validator.public_key,
                    )
                })
                .collect();

            self.mutate_db(|db, tx, pending| {
                db.insert_validator_tx(cluster.clone(), &validator, shares, tx, pending)
                    .expect("should insert validator");
            });
            cluster
        }

        /// Flip a cluster's liquidation status and publish the state update.
        fn set_cluster_liquidated(&self, cluster_id: ClusterId, liquidated: bool) {
            self.mutate_db(|db, tx, pending| {
                db.update_status_tx(cluster_id, liquidated, tx, pending)
                    .expect("should update cluster status");
            });
        }
    }

    #[tokio::test]
    async fn warmup_transition_subscribes_upcoming_fork_topics() {
        // Arrange
        let mut harness = TestHarness::new_normal_on_alan();
        // Startup in Normal(Alan) emits Alan subscriptions; clear them before the transition
        // assert.
        harness.consume_startup_events(SUBNET_COUNT).await;

        // Act
        harness.transition_to_warmup_alan_to_boole();
        let events = harness.recv_transition_events(SUBNET_COUNT).await;

        // Assert
        assert_all_events_match(&events, "Boole subscribe event", is_boole_subscribe);
        harness.assert_no_additional_events().await;
    }

    #[tokio::test]
    async fn grace_period_to_normal_unsubscribes_previous_fork_topics() {
        // Arrange
        let mut harness = TestHarness::new_grace_period_current_boole_previous_alan();
        // Startup in GracePeriod(current=Boole, previous=Alan) emits subscribes for both forks.
        harness.consume_startup_events(SUBNET_COUNT * 2).await;

        // Act
        harness.transition_to_normal_on_boole();
        let events = harness.recv_transition_events(SUBNET_COUNT).await;

        // Assert
        assert_all_events_match(&events, "Alan unsubscribe event", is_alan_unsubscribe);
        harness.assert_no_additional_events().await;
    }

    #[tokio::test]
    async fn warmup_to_grace_period_keeps_existing_topic_subscriptions() {
        // Arrange
        let mut harness = TestHarness::new_warmup_alan_to_boole();
        harness.consume_startup_events(SUBNET_COUNT * 2).await;

        // Act
        harness.transition_to_grace_period_current_boole_previous_alan();

        // Assert
        // WarmUp(Alan->Boole) and GracePeriod(current=Boole, previous=Alan) track the same fork
        // set, so no subscribe/unsubscribe topic events should be emitted for this
        // transition.
        harness.assert_no_additional_events().await;
    }

    #[tokio::test]
    async fn initial_lifecycle_warmup_subscribes_both_fork_topics() {
        // Arrange
        let mut harness = TestHarness::new_warmup_alan_to_boole();

        // Act
        let events = harness.recv_transition_events(SUBNET_COUNT * 2).await;

        // Assert
        assert_all_events_match(&events, "subscribe event", is_subscribe_event);
        let alan_subscribes = events
            .iter()
            .filter(|event| is_alan_subscribe(event))
            .count();
        let boole_subscribes = events
            .iter()
            .filter(|event| is_boole_subscribe(event))
            .count();

        assert_eq!(alan_subscribes, SUBNET_COUNT);
        assert_eq!(boole_subscribes, SUBNET_COUNT);
        harness.assert_no_additional_events().await;
    }

    #[tokio::test]
    async fn liquidating_cluster_unsubscribes_its_subnet() {
        // Arrange
        let mut harness = TestHarness::new_normal_on_alan_committee_subscriptions();
        // An empty database means startup emits no subscriptions.
        harness.assert_no_additional_events().await;

        let operators = test_operators();
        harness.insert_operators(&operators);
        let cluster = harness.insert_own_cluster(&operators, 1);
        let expected_subnet = alan_subnet_for(&operators);

        let events = harness.recv_transition_events(1).await;
        assert_alan_subscribe_for_subnet(&events[0], expected_subnet);
        harness.assert_no_additional_events().await;

        // Act
        harness.set_cluster_liquidated(cluster.cluster_id, true);

        // Assert
        let events = harness.recv_transition_events(1).await;
        assert_alan_unsubscribe_for_subnet(&events[0], expected_subnet);
        harness.assert_no_additional_events().await;
    }

    #[tokio::test]
    async fn reactivating_cluster_resubscribes_its_subnet() {
        // Arrange: reach the liquidated state with the cluster's subnet unsubscribed. Waiting
        // for each event before the next mutation prevents the watch channel from coalescing
        // consecutive updates into one no-op diff.
        let mut harness = TestHarness::new_normal_on_alan_committee_subscriptions();
        let operators = test_operators();
        harness.insert_operators(&operators);
        let cluster = harness.insert_own_cluster(&operators, 1);
        let expected_subnet = alan_subnet_for(&operators);
        let events = harness.recv_transition_events(1).await;
        assert_alan_subscribe_for_subnet(&events[0], expected_subnet);
        harness.set_cluster_liquidated(cluster.cluster_id, true);
        let events = harness.recv_transition_events(1).await;
        assert_alan_unsubscribe_for_subnet(&events[0], expected_subnet);

        // Act
        harness.set_cluster_liquidated(cluster.cluster_id, false);

        // Assert
        let events = harness.recv_transition_events(1).await;
        assert_alan_subscribe_for_subnet(&events[0], expected_subnet);
        harness.assert_no_additional_events().await;
    }

    #[tokio::test]
    async fn liquidating_one_of_two_clusters_sharing_subnet_keeps_subscription() {
        // Arrange
        let mut harness = TestHarness::new_normal_on_alan_committee_subscriptions();
        let operators = test_operators();
        harness.insert_operators(&operators);
        // Identical operator membership maps both clusters onto the same subnet, so the two
        // clusters produce a single subscribe event.
        let first_cluster = harness.insert_own_cluster(&operators, 1);
        let second_cluster = harness.insert_own_cluster(&operators, 2);
        let shared_subnet = alan_subnet_for(&operators);
        let events = harness.recv_transition_events(1).await;
        assert_alan_subscribe_for_subnet(&events[0], shared_subnet);
        harness.assert_no_additional_events().await;

        // Act
        harness.set_cluster_liquidated(first_cluster.cluster_id, true);

        // Assert: the remaining active cluster keeps the shared subnet subscribed. The quiet
        // window alone only proves no event arrived within the timeout, so flush the event
        // pipeline with a probe: insert a third own cluster on a different subnet and wait for
        // its subscribe. Events are delivered in order, so receiving it without a preceding
        // unsubscribe proves the first liquidation left the shared subnet untouched.
        harness.assert_no_additional_events().await;

        let probe_operators = probe_committee_operators(&operators[0]);
        let probe_subnet = alan_subnet_for(&probe_operators);
        assert_ne!(
            probe_subnet, shared_subnet,
            "probe committee must map to a different subnet for the ordering proof to hold"
        );
        // Our own operator already exists, so insert only the fresh ones.
        harness.insert_operators(&probe_operators[1..]);
        harness.insert_own_cluster(&probe_operators, 3);
        let events = harness.recv_transition_events(1).await;
        assert_alan_subscribe_for_subnet(&events[0], probe_subnet);

        // Liquidating the last active cluster on the shared subnet finally releases it.
        harness.set_cluster_liquidated(second_cluster.cluster_id, true);
        let events = harness.recv_transition_events(1).await;
        assert_alan_unsubscribe_for_subnet(&events[0], shared_subnet);
        harness.assert_no_additional_events().await;
    }

    #[tokio::test]
    async fn liquidated_cluster_excluded_from_subnet_message_rate() {
        // Arrange
        let harness = TestHarness::new_normal_on_alan_with_scoring();
        let operators = test_operators();
        harness.insert_operators(&operators);
        let cluster = harness.insert_own_cluster(&operators, 1);
        let subnet = alan_subnet_for(&operators);

        // The fixture validator carries a validator index, so the active cluster drives a
        // nonzero expected rate.
        let rate_before = harness
            .service
            .subnet_message_rate::<MinimalEthSpec>(
                &subnet,
                &harness.alan_config,
                &harness.db.state(),
            )
            .expect("scoring is enabled, so a rate should be produced");
        assert!(
            rate_before > 0.0,
            "active cluster should contribute a positive message rate, got {rate_before}"
        );

        // Act
        harness.set_cluster_liquidated(cluster.cluster_id, true);

        // Assert
        let rate_after = harness
            .service
            .subnet_message_rate::<MinimalEthSpec>(
                &subnet,
                &harness.alan_config,
                &harness.db.state(),
            )
            .expect("scoring is enabled, so a rate should be produced");
        assert_eq!(
            rate_after, 0.0,
            "liquidated cluster must not contribute to the expected message rate"
        );
    }

    fn normal_on_alan(alan_config: &ForkConfig, _boole_config: &ForkConfig) -> ForkLifecycle {
        ForkLifecycle::Normal {
            current: alan_config.clone(),
        }
    }

    fn normal_on_boole(_alan_config: &ForkConfig, boole_config: &ForkConfig) -> ForkLifecycle {
        ForkLifecycle::Normal {
            current: boole_config.clone(),
        }
    }

    fn warmup_from_alan_to_boole(
        alan_config: &ForkConfig,
        boole_config: &ForkConfig,
    ) -> ForkLifecycle {
        ForkLifecycle::WarmUp {
            current: alan_config.clone(),
            upcoming: boole_config.clone(),
        }
    }

    fn grace_period_current_boole_previous_alan(
        alan_config: &ForkConfig,
        boole_config: &ForkConfig,
    ) -> ForkLifecycle {
        ForkLifecycle::GracePeriod {
            current: boole_config.clone(),
            previous: alan_config.clone(),
        }
    }

    fn test_fork_schedule() -> (Arc<ForkSchedule>, ForkConfig, ForkConfig) {
        let mut configs = BTreeMap::new();
        configs.insert(Fork::Alan, (Epoch::new(0), ALAN_DOMAIN));
        configs.insert(Fork::Boole, (Epoch::new(BOOLE_FORK_EPOCH), BOOLE_DOMAIN));
        let schedule = Arc::new(
            ForkSchedule::from_fork_configs(configs, TEST_NETWORK)
                .expect("test fork schedule should be valid"),
        );
        let alan_config = schedule
            .config(Fork::Alan)
            .cloned()
            .expect("Alan config should exist");
        let boole_config = schedule
            .config(Fork::Boole)
            .cloned()
            .expect("Boole config should exist");
        (schedule, alan_config, boole_config)
    }

    fn boole_topic_prefix() -> &'static str {
        static PREFIX: OnceLock<String> = OnceLock::new();
        PREFIX.get_or_init(|| Fork::Boole.topic_prefix(TEST_NETWORK))
    }

    fn describe_topic_event(event: &TopicEvent) -> String {
        match event {
            TopicEvent::Subscribe { topic, .. } => format!("subscribe({topic})"),
            TopicEvent::Unsubscribe { topic, .. } => format!("unsubscribe({topic})"),
            TopicEvent::RateUpdate { topic, .. } => format!("rate_update({topic})"),
        }
    }

    fn assert_all_events_match(
        events: &[TopicEvent],
        expected_event: &str,
        predicate: impl Fn(&TopicEvent) -> bool,
    ) {
        for (index, event) in events.iter().enumerate() {
            assert!(
                predicate(event),
                "unexpected event at index {index}: expected {expected_event}, got {}",
                describe_topic_event(event)
            );
        }
    }

    fn is_subscribe_event(event: &TopicEvent) -> bool {
        matches!(event, TopicEvent::Subscribe { .. })
    }

    fn is_alan_subscribe(event: &TopicEvent) -> bool {
        matches!(
            event,
            TopicEvent::Subscribe { topic, .. } if topic.starts_with(ALAN_TOPIC_PREFIX)
        )
    }

    fn is_boole_subscribe(event: &TopicEvent) -> bool {
        matches!(
            event,
            TopicEvent::Subscribe { topic, .. } if topic.starts_with(boole_topic_prefix())
        )
    }

    fn is_alan_unsubscribe(event: &TopicEvent) -> bool {
        matches!(
            event,
            TopicEvent::Unsubscribe { topic, .. } if topic.starts_with(ALAN_TOPIC_PREFIX)
        )
    }

    /// Operators with consecutive ids starting at `OWN_OPERATOR_ID`, so clusters generated from
    /// them become own clusters.
    fn test_operators() -> Vec<Operator> {
        (OWN_OPERATOR_ID.0..OWN_OPERATOR_ID.0 + DEFAULT_NUM_OPERATORS)
            .map(generators::operator::with_id)
            .collect()
    }

    /// A committee that reuses our own operator (so its cluster is still an own cluster) but is
    /// otherwise made of fresh operators with ids disjoint from `test_operators`, mapping it to
    /// a different subnet. Callers must insert the fresh operators (all but the first) before
    /// inserting a cluster of these operators.
    fn probe_committee_operators(own_operator: &Operator) -> Vec<Operator> {
        let fresh_id_start = OWN_OPERATOR_ID.0 + DEFAULT_NUM_OPERATORS;
        let mut operators = vec![own_operator.clone()];
        operators.extend(
            (fresh_id_start..fresh_id_start + DEFAULT_NUM_OPERATORS - 1)
                .map(generators::operator::with_id),
        );
        operators
    }

    /// Subnet that an Alan-fork committee of these operators maps to.
    fn alan_subnet_for(operators: &[Operator]) -> SubnetId {
        let members: Vec<OperatorId> = operators.iter().map(|operator| operator.id).collect();
        SubnetId::from_operators_for_fork(&members, Fork::Alan)
            .expect("should calculate subnet for committee")
    }

    fn assert_alan_subscribe_for_subnet(event: &TopicEvent, expected_subnet: SubnetId) {
        match event {
            TopicEvent::Subscribe { topic, subnet, .. }
                if topic.starts_with(ALAN_TOPIC_PREFIX) && *subnet == expected_subnet => {}
            other => panic!(
                "expected Alan subscribe for subnet {}, got {}",
                *expected_subnet,
                describe_topic_event(other)
            ),
        }
    }

    fn assert_alan_unsubscribe_for_subnet(event: &TopicEvent, expected_subnet: SubnetId) {
        match event {
            TopicEvent::Unsubscribe { topic, subnet }
                if topic.starts_with(ALAN_TOPIC_PREFIX) && *subnet == expected_subnet => {}
            other => panic!(
                "expected Alan unsubscribe for subnet {}, got {}",
                *expected_subnet,
                describe_topic_event(other)
            ),
        }
    }
}

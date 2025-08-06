pub mod cli;
pub mod config;
mod key;
mod notifier;

use std::{
    fs::File,
    io::Read,
    net::SocketAddr,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anchor_validator_store::{AnchorValidatorStore, metadata_service::MetadataService};
use beacon_node_fallback::{
    ApiTopic, BeaconNodeFallback, CandidateBeaconNode, start_fallback_updater_service,
};
pub use cli::Node;
use config::Config;
use database::{NetworkDatabase, OwnOperatorId};
use duties_tracker::{duties_tracker::DutiesTracker, voluntary_exit_tracker::VoluntaryExitTracker};
use eth::{
    index_sync::start_validator_index_syncer, voluntary_exit_processor::start_exit_processor,
};
use eth2::{
    BeaconNodeHttpClient, Timeouts,
    reqwest::{Certificate, ClientBuilder},
};
use message_receiver::NetworkMessageReceiver;
use message_sender::{MessageSender, NetworkMessageSender, impostor::ImpostorMessageSender};
use message_validator::Validator;
use network::Network;
use openssl::rsa::Rsa;
use parking_lot::RwLock;
use qbft_manager::QbftManager;
use sensitive_url::SensitiveUrl;
use signature_collector::SignatureCollectorManager;
use slashing_protection::SlashingDatabase;
use slot_clock::{SlotClock, SystemTimeSlotClock};
use subnet_service::{SUBNET_COUNT, SubnetId, start_subnet_service};
use task_executor::TaskExecutor;
use tokio::{
    net::TcpListener,
    select,
    sync::{mpsc, mpsc::unbounded_channel},
    time::{Instant, interval, sleep},
};
use tracing::{debug, error, info, warn};
use types::{EthSpec, Hash256};
use validator_metrics::set_gauge;
use validator_services::{
    attestation_service::AttestationServiceBuilder,
    block_service::BlockServiceBuilder,
    duties_service,
    duties_service::{DutiesServiceBuilder, SelectionProofConfig},
    latency_service::start_latency_service,
    preparation_service::PreparationServiceBuilder,
    sync_committee_service::SyncCommitteeService,
};

use crate::{key::read_or_generate_private_key, notifier::spawn_notifier};

/// The filename within the `validators` directory that contains the slashing protection DB.
const SLASHING_PROTECTION_FILENAME: &str = "slashing_protection.sqlite";

/// Specific timeout constants for HTTP requests involved in different validator duties.
/// This can help ensure that proper endpoint fallback occurs.
const HTTP_ATTESTATION_TIMEOUT_QUOTIENT: u32 = 4;
const HTTP_ATTESTER_DUTIES_TIMEOUT_QUOTIENT: u32 = 4;
const HTTP_ATTESTATION_SUBSCRIPTIONS_TIMEOUT_QUOTIENT: u32 = 24;
const HTTP_LIVENESS_TIMEOUT_QUOTIENT: u32 = 4;
const HTTP_PROPOSAL_TIMEOUT_QUOTIENT: u32 = 2;
const HTTP_PROPOSER_DUTIES_TIMEOUT_QUOTIENT: u32 = 4;
const HTTP_SYNC_COMMITTEE_CONTRIBUTION_TIMEOUT_QUOTIENT: u32 = 4;
const HTTP_SYNC_DUTIES_TIMEOUT_QUOTIENT: u32 = 4;
const HTTP_GET_BEACON_BLOCK_SSZ_TIMEOUT_QUOTIENT: u32 = 4;
const HTTP_GET_DEBUG_BEACON_STATE_QUOTIENT: u32 = 4;
const HTTP_GET_DEPOSIT_SNAPSHOT_QUOTIENT: u32 = 4;
const HTTP_GET_VALIDATOR_BLOCK_TIMEOUT_QUOTIENT: u32 = 4;
const HTTP_DEFAULT_TIMEOUT_QUOTIENT: u32 = 4;

const MAINNET_GENESIS_FORK_VERSION: [u8; 4] = [0, 0, 0, 0];

pub struct Client {}

impl Client {
    /// Runs the Anchor Client
    pub async fn run<E: EthSpec>(executor: TaskExecutor, config: Config) -> Result<(), String> {
        // Attempt to raise soft fd limit. The behavior is OS specific:
        // `linux` - raise soft fd limit to hard
        // `macos` - raise soft fd limit to `min(kernel limit, hard fd limit)`
        // `windows` & rest - noop
        match fdlimit::raise_fd_limit().map_err(|e| format!("Unable to raise fd limit: {e}"))? {
            fdlimit::Outcome::LimitRaised { from, to } => {
                debug!(
                    old_limit = from,
                    new_limit = to,
                    "Raised soft open file descriptor resource limit"
                );
            }
            fdlimit::Outcome::Unsupported => {
                debug!("Raising soft open file descriptor resource limit is not supported");
            }
        };

        info!(
            beacon_nodes = format!("{:?}", &config.beacon_nodes),
            execution_nodes = format!("{:?}", &config.execution_nodes),
            execution_nodes_websocket = format!("{:?}", &config.execution_nodes_websocket),
            data_dir = format!("{:?}", config.global_config.data_dir),
            "Starting the Anchor client"
        );

        let spec = Arc::new(
            config
                .global_config
                .ssv_network
                .eth2_network
                .chain_spec::<E>()?,
        );

        if spec.genesis_fork_version == MAINNET_GENESIS_FORK_VERSION {
            return Err(
                "Mainnet is not supported. Please use a testnet configuration.".to_string(),
            );
        }

        let key = read_or_generate_private_key(
            &config.global_config.data_dir,
            config.key_file.as_deref(),
            config.password_file.as_deref(),
        )?;
        let err = |e| format!("Unable to derive public key: {e:?}");
        let pubkey = Rsa::from_public_components(
            key.n().to_owned().map_err(err)?,
            key.e().to_owned().map_err(err)?,
        )
        .map_err(err)?;

        // Start the processor
        let processor_senders = processor::spawn(config.processor, executor.clone());

        // Optionally start the metrics server.
        let http_metrics_shared_state = if config.http_metrics.enabled {
            let shared_state = Arc::new(RwLock::new(http_metrics::Shared {
                genesis_time: None,
                duties_service: None,
                network_registry: None,
            }));

            let exit = executor.exit();

            // Attempt to bind to the socket
            let socket = SocketAddr::new(
                config.http_metrics.listen_addr,
                config.http_metrics.listen_port,
            );
            let listener = TcpListener::bind(socket)
                .await
                .map_err(|e| format!("Unable to bind to metrics server port: {e}"))?;

            let metrics_future = http_metrics::serve(listener, shared_state.clone(), exit);

            executor.spawn_without_exit(metrics_future, "metrics-http");
            Some(shared_state)
        } else {
            info!("HTTP metrics server is disabled");
            None
        };

        // Optionally run the http_api server
        let http_api_shared_state = Arc::new(RwLock::new(http_api::Shared {
            database_state: None,
        }));
        let state = http_api_shared_state.clone();

        executor.spawn(
            async {
                if let Err(error) = http_api::run(config.http_api, state).await {
                    error!(error, "Failed to run HTTP API");
                }
            },
            "http_api_server",
        );

        // Open database
        let database = Arc::new(
            if let Some(impostor) = &config.impostor {
                NetworkDatabase::new_as_impostor(
                    config
                        .global_config
                        .data_dir
                        .join("anchor_db.sqlite")
                        .as_path(),
                    impostor,
                )
            } else {
                NetworkDatabase::new(
                    config
                        .global_config
                        .data_dir
                        .join("anchor_db.sqlite")
                        .as_path(),
                    &pubkey,
                )
            }
            .map_err(|e| format!("Unable to open Anchor database: {e}"))?,
        );

        // Initialize slashing protection.
        let slashing_db_path = config
            .global_config
            .data_dir
            .join(SLASHING_PROTECTION_FILENAME);
        let slashing_protection =
            SlashingDatabase::open_or_create(&slashing_db_path).map_err(|e| {
                format!("Failed to open or create slashing protection database: {e:?}",)
            })?;

        let last_beacon_node_index = config
            .beacon_nodes
            .len()
            .checked_sub(1)
            .ok_or_else(|| "No beacon nodes defined.".to_string())?;

        let beacon_node_setup = |x: (usize, &SensitiveUrl)| {
            let i = x.0;
            let url = x.1;
            let slot_duration = Duration::from_secs(spec.seconds_per_slot);

            let mut beacon_node_http_client_builder = ClientBuilder::new();

            // Add new custom root certificates if specified.
            if let Some(certificates) = &config.beacon_nodes_tls_certs {
                for cert in certificates {
                    beacon_node_http_client_builder = beacon_node_http_client_builder
                        .add_root_certificate(load_pem_certificate(cert)?);
                }
            }

            let beacon_node_http_client = beacon_node_http_client_builder
                // Set default timeout to be the full slot duration.
                .timeout(slot_duration)
                .build()
                .map_err(|e| format!("Unable to build HTTP client: {e:?}"))?;

            // Use quicker timeouts if a fallback beacon node exists.
            let timeouts = if i < last_beacon_node_index && !config.use_long_timeouts {
                info!("Fallback endpoints are available, using optimized timeouts.");
                Timeouts {
                    attestation: slot_duration / HTTP_ATTESTATION_TIMEOUT_QUOTIENT,
                    attester_duties: slot_duration / HTTP_ATTESTER_DUTIES_TIMEOUT_QUOTIENT,
                    attestation_subscriptions: slot_duration
                        / HTTP_ATTESTATION_SUBSCRIPTIONS_TIMEOUT_QUOTIENT,
                    attestation_aggregators: slot_duration / HTTP_ATTESTATION_TIMEOUT_QUOTIENT,
                    liveness: slot_duration / HTTP_LIVENESS_TIMEOUT_QUOTIENT,
                    proposal: slot_duration / HTTP_PROPOSAL_TIMEOUT_QUOTIENT,
                    proposer_duties: slot_duration / HTTP_PROPOSER_DUTIES_TIMEOUT_QUOTIENT,
                    sync_committee_contribution: slot_duration
                        / HTTP_SYNC_COMMITTEE_CONTRIBUTION_TIMEOUT_QUOTIENT,
                    sync_duties: slot_duration / HTTP_SYNC_DUTIES_TIMEOUT_QUOTIENT,
                    sync_aggregators: slot_duration / HTTP_SYNC_DUTIES_TIMEOUT_QUOTIENT,
                    get_beacon_blocks_ssz: slot_duration
                        / HTTP_GET_BEACON_BLOCK_SSZ_TIMEOUT_QUOTIENT,
                    get_debug_beacon_states: slot_duration / HTTP_GET_DEBUG_BEACON_STATE_QUOTIENT,
                    get_deposit_snapshot: slot_duration / HTTP_GET_DEPOSIT_SNAPSHOT_QUOTIENT,
                    get_validator_block: slot_duration / HTTP_GET_VALIDATOR_BLOCK_TIMEOUT_QUOTIENT,
                    default: slot_duration / HTTP_DEFAULT_TIMEOUT_QUOTIENT,
                }
            } else {
                Timeouts::set_all(slot_duration)
            };

            Ok(BeaconNodeHttpClient::from_components(
                url.clone(),
                beacon_node_http_client,
                timeouts,
            ))
        };

        let beacon_nodes: Vec<BeaconNodeHttpClient> = config
            .beacon_nodes
            .iter()
            .enumerate()
            .map(beacon_node_setup)
            .collect::<Result<Vec<BeaconNodeHttpClient>, String>>()?;

        let proposer_nodes: Vec<BeaconNodeHttpClient> = config
            .proposer_nodes
            .iter()
            .enumerate()
            .map(beacon_node_setup)
            .collect::<Result<Vec<BeaconNodeHttpClient>, String>>()?;

        let num_nodes = beacon_nodes.len();
        // User order of `beacon_nodes` is preserved, so `index` corresponds to the position of
        // the node in `--beacon_nodes`.
        let candidates = beacon_nodes
            .into_iter()
            .enumerate()
            .map(|(index, node)| CandidateBeaconNode::new(node, index))
            .collect();

        // User order of `proposer_nodes` is preserved, so `index` corresponds to the position of
        // the node in `--proposer_nodes`.
        let proposer_candidates = proposer_nodes
            .into_iter()
            .enumerate()
            .map(|(index, node)| CandidateBeaconNode::new(node, index))
            .collect();

        // Set the count for beacon node fallbacks excluding the primary beacon node.
        set_gauge(
            &validator_metrics::ETH2_FALLBACK_CONFIGURED,
            num_nodes.saturating_sub(1) as i64,
        );
        // Set the total beacon node count.
        set_gauge(
            &validator_metrics::TOTAL_BEACON_NODES_COUNT,
            num_nodes as i64,
        );

        // Initialize the number of connected, synced beacon nodes to 0.
        set_gauge(&validator_metrics::ETH2_FALLBACK_CONNECTED, 0);
        set_gauge(&validator_metrics::SYNCED_BEACON_NODES_COUNT, 0);
        // Initialize the number of connected, avaliable beacon nodes to 0.
        set_gauge(&validator_metrics::AVAILABLE_BEACON_NODES_COUNT, 0);

        // TODO: make beacon_node_fallback::Config and broadcast_topics configurable
        // https://github.com/sigp/anchor/issues/248
        let mut beacon_nodes: BeaconNodeFallback<_> = BeaconNodeFallback::new(
            candidates,
            beacon_node_fallback::Config::default(),
            vec![ApiTopic::Subscriptions],
            spec.clone(),
        );

        let mut proposer_nodes: BeaconNodeFallback<_> = BeaconNodeFallback::new(
            proposer_candidates,
            beacon_node_fallback::Config::default(),
            vec![ApiTopic::Subscriptions],
            spec.clone(),
        );

        // Perform some potentially long-running initialization tasks.
        let (genesis_time, genesis_validators_root) = tokio::select! {
            tuple = init_from_beacon_node::<E>(&beacon_nodes, &proposer_nodes) => tuple?,
            () = executor.exit() => return Err("Shutting down".to_string())
        };

        let slot_clock = SystemTimeSlotClock::new(
            spec.genesis_slot,
            Duration::from_secs(genesis_time),
            Duration::from_secs(spec.seconds_per_slot),
        );

        beacon_nodes.set_slot_clock(slot_clock.clone());
        proposer_nodes.set_slot_clock(slot_clock.clone());

        let beacon_nodes = Arc::new(beacon_nodes);
        start_fallback_updater_service::<_, E>(executor.clone(), beacon_nodes.clone())?;

        let proposer_nodes = Arc::new(proposer_nodes);
        start_fallback_updater_service::<_, E>(executor.clone(), proposer_nodes.clone())?;

        // Wait until genesis has occurred.
        wait_for_genesis(genesis_time).await?;

        // Start validator index syncer
        let index_sync_tx =
            start_validator_index_syncer(beacon_nodes.clone(), database.clone(), executor.clone());

        // We create the channel here so that we can pass the receiver to the syncer. But we need to
        // delay starting the voluntary exit processor until we have created the validator store.
        let (exit_tx, exit_rx) = unbounded_channel();
        let voluntary_exit_tracker = Arc::new(VoluntaryExitTracker::new());

        // Start syncer
        let mut syncer = eth::SsvEventSyncer::new(
            database.clone(),
            index_sync_tx,
            exit_tx,
            eth::Config {
                http_urls: config.execution_nodes,
                ws_url: config.execution_nodes_websocket,
                network: config.global_config.ssv_network.clone(),
            },
        )
        .await
        .map_err(|e| format!("Unable to create syncer: {e}"))?;

        // Access to the sync status. This can be passed around to condition duties based on whether
        // we are synced.
        let is_synced = syncer.is_synced();

        executor.spawn(
            async move {
                if let Err(e) = syncer.sync().await {
                    error!("Syncer failed: {e}");
                }
            },
            "syncer",
        );

        let operator_id = OwnOperatorId::new(database.watch());

        // Network sender/receiver
        let (network_tx, network_rx) = mpsc::channel::<(SubnetId, Vec<u8>)>(9001);

        let duties_tracker = Arc::new(DutiesTracker::new(
            voluntary_exit_tracker.clone(),
            beacon_nodes.clone(),
            spec.clone(),
            E::slots_per_epoch(),
            slot_clock.clone(),
            database.watch(),
        ));
        duties_tracker.clone().start(executor.clone());

        let message_validator = Validator::new(
            database.watch(),
            E::slots_per_epoch(),
            spec.epochs_per_sync_committee_period.as_u64(),
            E::sync_committee_size(),
            duties_tracker.clone(),
            slot_clock.clone(),
            &executor,
        );

        let message_sender: Arc<dyn MessageSender> = if config.impostor.is_none() {
            Arc::new(NetworkMessageSender::new(
                processor_senders.clone(),
                network_tx.clone(),
                key.clone(),
                operator_id.clone(),
                Some(message_validator.clone()),
                SUBNET_COUNT,
                is_synced.clone(),
            )?)
        } else {
            Arc::new(ImpostorMessageSender::new(network_tx.clone(), SUBNET_COUNT))
        };

        // Create the signature collector
        let signature_collector = SignatureCollectorManager::new(
            processor_senders.clone(),
            operator_id.clone(),
            config.global_config.ssv_network.ssv_domain_type.clone(),
            message_sender.clone(),
            slot_clock.clone(),
        )
        .map_err(|e| format!("Unable to initialize signature collector manager: {e:?}"))?;

        // Create the qbft manager
        let qbft_manager = QbftManager::new(
            processor_senders.clone(),
            operator_id.clone(),
            slot_clock.clone(),
            message_sender,
            config.global_config.ssv_network.ssv_domain_type.clone(),
        )
        .map_err(|e| format!("Unable to initialize qbft manager: {e:?}"))?;

        // Start the subnet service now that we have slot_clock
        let subnet_service = start_subnet_service::<E>(
            database.watch(),
            SUBNET_COUNT,
            config.network.subscribe_all_subnets,
            config.network.disable_gossipsub_topic_scoring,
            &executor,
            slot_clock.clone(),
            spec.clone(),
        );

        let (outcome_tx, outcome_rx) = mpsc::channel::<message_receiver::Outcome>(9000);

        let message_receiver = NetworkMessageReceiver::new(
            processor_senders.clone(),
            qbft_manager.clone(),
            signature_collector.clone(),
            database.watch(),
            outcome_tx,
            message_validator,
        );

        // Start the p2p network
        let mut network = Network::try_new::<E>(
            &config.network,
            subnet_service,
            network_rx,
            Arc::new(message_receiver),
            outcome_rx,
            executor.clone(),
            spec.clone(),
        )
        .await
        .map_err(|e| format!("Unable to start network: {e}"))?;

        let network_metrics_registry = network.take_metrics_registry();
        if let Some(metrics_state) = &http_metrics_shared_state {
            metrics_state.write().network_registry = network_metrics_registry;
        }

        // Spawn the network listening task
        executor.spawn(network.run::<E>(), "network");

        let validator_store = AnchorValidatorStore::<_, E>::new(
            database.watch(),
            signature_collector,
            qbft_manager,
            slashing_protection,
            config.disable_slashing_protection,
            slot_clock.clone(),
            spec.clone(),
            genesis_validators_root,
            config.impostor.is_none().then_some(key),
            executor.clone(),
            config.gas_limit,
            config.builder_proposals,
            config.builder_boost_factor,
            config.prefer_builder_proposals,
            is_synced.clone(),
        );

        start_exit_processor(
            slot_clock.clone(),
            E::slots_per_epoch(),
            beacon_nodes.clone(),
            validator_store.clone(),
            exit_rx,
            executor.clone(),
            voluntary_exit_tracker.clone(),
        );

        let selection_proof_config = SelectionProofConfig {
            lookahead_slot: 0,
            computation_offset: Duration::ZERO,
            selections_endpoint: false,
            parallel_sign: true,
        };

        let duties_service = Arc::new(
            DutiesServiceBuilder::new()
                .slot_clock(slot_clock.clone())
                .beacon_nodes(beacon_nodes.clone())
                .validator_store(validator_store.clone())
                .spec(spec.clone())
                .executor(executor.clone())
                .enable_high_validator_count_metrics(config.enable_high_validator_count_metrics)
                .attestation_selection_proof_config(selection_proof_config)
                .sync_selection_proof_config(selection_proof_config)
                .build()?,
        );

        // Update the metrics server.
        if let Some(ctx) = &http_metrics_shared_state {
            ctx.write().genesis_time = Some(genesis_time);
            ctx.write().duties_service = Some(duties_service.clone());
        }

        // Spawn notifier for logging and metrics
        spawn_notifier(
            duties_service.clone(),
            database.watch(),
            is_synced.clone(),
            executor.clone(),
            &spec,
        );

        // Wait for sync to complete before starting services
        info!("Waiting for sync to complete before starting services...");
        is_synced
            .clone()
            .wait_for(|&is_synced| is_synced)
            .await
            .map_err(|_| "Sync watch channel closed")?;
        info!("Sync complete, starting services...");

        let mut block_service_builder = BlockServiceBuilder::new()
            .slot_clock(slot_clock.clone())
            .validator_store(validator_store.clone())
            .beacon_nodes(beacon_nodes.clone())
            .executor(executor.clone())
            .chain_spec(spec.clone());

        // If we have proposer nodes, add them to the block service builder.
        if proposer_nodes.num_total().await > 0 {
            block_service_builder = block_service_builder.proposer_nodes(proposer_nodes.clone());
        }

        let block_service = block_service_builder.build()?;

        let attestation_service = AttestationServiceBuilder::new()
            .duties_service(duties_service.clone())
            .slot_clock(slot_clock.clone())
            .validator_store(validator_store.clone())
            .beacon_nodes(beacon_nodes.clone())
            .executor(executor.clone())
            .chain_spec(spec.clone())
            .build()?;

        let preparation_service = PreparationServiceBuilder::new()
            .slot_clock(slot_clock.clone())
            .validator_store(validator_store.clone())
            .beacon_nodes(beacon_nodes.clone())
            .executor(executor.clone())
            .validator_registration_batch_size(500)
            .build()?;

        let sync_committee_service = SyncCommitteeService::new(
            duties_service.clone(),
            validator_store.clone(),
            slot_clock.clone(),
            beacon_nodes.clone(),
            executor.clone(),
        );

        let metadata_service = MetadataService::new(
            duties_service.clone(),
            validator_store.clone(),
            slot_clock.clone(),
            beacon_nodes.clone(),
            executor.clone(),
            spec.clone(),
        );

        // We use `SLOTS_PER_EPOCH` as the capacity of the block notification channel, because
        // we don't expect notifications to be delayed by more than a single slot, let alone a
        // whole epoch!
        let channel_capacity = E::slots_per_epoch() as usize;
        let (block_service_tx, block_service_rx) = mpsc::channel(channel_capacity);

        duties_service::start_update_service(duties_service.clone(), block_service_tx);

        block_service
            .start_update_service(block_service_rx)
            .map_err(|e| format!("Unable to start block service: {e}"))?;

        attestation_service
            .start_update_service(&spec)
            .map_err(|e| format!("Unable to start attestation service: {e}"))?;

        sync_committee_service
            .start_update_service(&spec)
            .map_err(|e| format!("Unable to start sync committee service: {e}"))?;

        metadata_service
            .start_update_service()
            .map_err(|e| format!("Unable to start metadata service: {e}"))?;

        preparation_service
            .start_update_service(&spec)
            .map_err(|e| format!("Unable to start preparation service: {e}"))?;

        http_api_shared_state.write().database_state = Some(database.watch());

        if !config.disable_latency_measurement_service {
            start_latency_service(executor.clone(), slot_clock.clone(), beacon_nodes.clone());
        }

        Ok(())
    }
}

async fn init_from_beacon_node<E: EthSpec>(
    beacon_nodes: &BeaconNodeFallback<SystemTimeSlotClock>,
    proposer_nodes: &BeaconNodeFallback<SystemTimeSlotClock>,
) -> Result<(u64, Hash256), String> {
    const RETRY_DELAY: Duration = Duration::from_secs(2);

    loop {
        beacon_nodes.update_all_candidates::<E>().await;
        proposer_nodes.update_all_candidates::<E>().await;

        let num_available = beacon_nodes.num_available().await;
        let num_total = beacon_nodes.num_total().await;

        let proposer_available = proposer_nodes.num_available().await;
        let proposer_total = proposer_nodes.num_total().await;

        if proposer_total > 0 && proposer_available == 0 {
            warn!(
                retry_in = format!("{} seconds", RETRY_DELAY.as_secs()),
                total_proposers = proposer_total,
                available_proposers = proposer_available,
                total_beacon_nodes = num_total,
                available_beacon_nodes = num_available,
                "Unable to connect to a proposer node"
            );
        }

        if num_available > 0 && proposer_available == 0 {
            info!(
                total = num_total,
                available = num_available,
                "Initialized beacon node connections"
            );
            break;
        } else if num_available > 0 {
            info!(
                total = num_total,
                available = num_available,
                proposers_available = proposer_available,
                proposers_total = proposer_total,
                "Initialized beacon node connections"
            );
            break;
        } else {
            warn!(
                retry_in = format!("{} seconds", RETRY_DELAY.as_secs()),
                total = num_total,
                available = num_available,
                "Unable to connect to a beacon node"
            );
            sleep(RETRY_DELAY).await;
        }
    }

    let genesis = loop {
        match beacon_nodes
            .first_success(|node| async move { node.get_beacon_genesis().await })
            .await
        {
            Ok(genesis) => break genesis.data,
            Err(errors) => {
                // Search for a 404 error which indicates that genesis has not yet
                // occurred.
                if errors
                    .0
                    .iter()
                    .filter_map(|(_, e)| e.request_failure())
                    .any(|e| e.status() == Some(eth2::StatusCode::NOT_FOUND))
                {
                    info!("Waiting for genesis");
                } else {
                    error!(
                        error = ?errors.0,
                        "Errors polling beacon node",
                    );
                }
            }
        }

        sleep(RETRY_DELAY).await;
    };

    Ok((genesis.genesis_time, genesis.genesis_validators_root))
}

async fn wait_for_genesis(genesis_time: u64) -> Result<(), String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("Unable to read system time: {e:?}"))?;
    let genesis_time = Duration::from_secs(genesis_time);

    // Sleep until genesis, or not at all if `now >= genesis_time`
    let genesis_sleep = sleep(genesis_time.saturating_sub(now));
    tokio::pin!(genesis_sleep);
    let mut log_interval = interval(Duration::from_secs(30));

    loop {
        select! {
            biased;
            _ = &mut genesis_sleep => break,
            _ = log_interval.tick() => {
                let seconds_to_wait = genesis_sleep.deadline().duration_since(Instant::now()).as_secs();
                info!(seconds_to_wait, "Waiting for genesis");
            },
        }
    }

    info!("Genesis has occurred");
    Ok(())
}

pub fn load_pem_certificate<P: AsRef<Path>>(pem_path: P) -> Result<Certificate, String> {
    let mut buf = Vec::new();
    File::open(&pem_path)
        .map_err(|e| format!("Unable to open certificate path: {e}"))?
        .read_to_end(&mut buf)
        .map_err(|e| format!("Unable to read certificate file: {e}"))?;
    Certificate::from_pem(&buf).map_err(|e| format!("Unable to parse certificate: {e}"))
}

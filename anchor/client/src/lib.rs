// use tracing::{debug, info};

mod cli;
pub mod config;

use anchor_validator_store::sync_committee_service::SyncCommitteeService;
use anchor_validator_store::AnchorValidatorStore;
use beacon_node_fallback::{
    start_fallback_updater_service, ApiTopic, BeaconNodeFallback, CandidateBeaconNode,
};
pub use cli::Anchor;
use config::Config;
use eth2::reqwest::{Certificate, ClientBuilder};
use eth2::{BeaconNodeHttpClient, Timeouts};
use eth2_config::Eth2Config;
use network::Network;
use parking_lot::RwLock;
use qbft_manager::QbftManager;
use sensitive_url::SensitiveUrl;
use signature_collector::SignatureCollectorManager;
use slashing_protection::SlashingDatabase;
use slot_clock::{SlotClock, SystemTimeSlotClock};
use ssv_types::OperatorId;
use std::fs::File;
use std::io::Read;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use task_executor::TaskExecutor;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};
use types::{EthSpec, Hash256};
use validator_metrics::set_gauge;
use validator_services::attestation_service::AttestationServiceBuilder;
use validator_services::block_service::BlockServiceBuilder;
use validator_services::duties_service;
use validator_services::duties_service::DutiesServiceBuilder;
use validator_services::preparation_service::PreparationServiceBuilder;

/// The filename within the `validators` directory that contains the slashing protection DB.
const SLASHING_PROTECTION_FILENAME: &str = "slashing_protection.sqlite";

/// The time between polls when waiting for genesis.
const WAITING_FOR_GENESIS_POLL_TIME: Duration = Duration::from_secs(12);

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

pub struct Client {}

impl Client {
    /// Runs the Anchor Client
    pub async fn run<E: EthSpec>(executor: TaskExecutor, config: Config) -> Result<(), String> {
        // Attempt to raise soft fd limit. The behavior is OS specific:
        // `linux` - raise soft fd limit to hard
        // `macos` - raise soft fd limit to `min(kernel limit, hard fd limit)`
        // `windows` & rest - noop
        match fdlimit::raise_fd_limit().map_err(|e| format!("Unable to raise fd limit: {}", e))? {
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
            data_dir = format!("{:?}", config.data_dir),
            "Starting the Anchor client"
        );

        // TODO make configurable
        let spec = Eth2Config::mainnet().spec;

        // Optionally start the metrics server.
        let http_metrics_shared_state = if config.http_metrics.enabled {
            let shared_state = Arc::new(RwLock::new(http_metrics::Shared { genesis_time: None }));

            let exit = executor.exit();

            // Attempt to bind to the socket
            let socket = SocketAddr::new(config.http_api.listen_addr, config.http_api.listen_port);
            let listener = TcpListener::bind(socket)
                .await
                .map_err(|e| format!("Unable to bind to metrics server port: {}", e))?;

            let metrics_future = http_metrics::serve(listener, shared_state.clone(), exit);

            executor.spawn_without_exit(metrics_future, "metrics-http");
            Some(shared_state)
        } else {
            info!("HTTP metrics server is disabled");
            None
        };

        // Optionally run the http_api server
        if let Err(error) = http_api::run(config.http_api).await {
            error!(error, "Failed to run HTTP API");
            return Err("HTTP API Failed".to_string());
        }

        // Start the p2p network
        let network = Network::try_new(&config.network, executor.clone()).await?;
        // Spawn the network listening task
        executor.spawn(network.run(), "network");

        // Initialize slashing protection.
        let slashing_db_path = config.data_dir.join(SLASHING_PROTECTION_FILENAME);
        let slashing_protection =
            SlashingDatabase::open_or_create(&slashing_db_path).map_err(|e| {
                format!(
                    "Failed to open or create slashing protection database: {:?}",
                    e
                )
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
                .map_err(|e| format!("Unable to build HTTP client: {:?}", e))?;

            // Use quicker timeouts if a fallback beacon node exists.
            let timeouts = if i < last_beacon_node_index && !config.use_long_timeouts {
                info!("Fallback endpoints are available, using optimized timeouts.");
                Timeouts {
                    attestation: slot_duration / HTTP_ATTESTATION_TIMEOUT_QUOTIENT,
                    attester_duties: slot_duration / HTTP_ATTESTER_DUTIES_TIMEOUT_QUOTIENT,
                    attestation_subscriptions: slot_duration
                        / HTTP_ATTESTATION_SUBSCRIPTIONS_TIMEOUT_QUOTIENT,
                    liveness: slot_duration / HTTP_LIVENESS_TIMEOUT_QUOTIENT,
                    proposal: slot_duration / HTTP_PROPOSAL_TIMEOUT_QUOTIENT,
                    proposer_duties: slot_duration / HTTP_PROPOSER_DUTIES_TIMEOUT_QUOTIENT,
                    sync_committee_contribution: slot_duration
                        / HTTP_SYNC_COMMITTEE_CONTRIBUTION_TIMEOUT_QUOTIENT,
                    sync_duties: slot_duration / HTTP_SYNC_DUTIES_TIMEOUT_QUOTIENT,
                    get_beacon_blocks_ssz: slot_duration
                        / HTTP_GET_BEACON_BLOCK_SSZ_TIMEOUT_QUOTIENT,
                    get_debug_beacon_states: slot_duration / HTTP_GET_DEBUG_BEACON_STATE_QUOTIENT,
                    get_deposit_snapshot: slot_duration / HTTP_GET_DEPOSIT_SNAPSHOT_QUOTIENT,
                    get_validator_block: slot_duration / HTTP_GET_VALIDATOR_BLOCK_TIMEOUT_QUOTIENT,
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

        let mut beacon_nodes: BeaconNodeFallback<_> = BeaconNodeFallback::new(
            candidates,
            beacon_node_fallback::Config::default(), // TODO make configurable
            vec![ApiTopic::Subscriptions],           // TODO make configurable
            spec.clone(),
        );

        let mut proposer_nodes: BeaconNodeFallback<_> = BeaconNodeFallback::new(
            proposer_candidates,
            beacon_node_fallback::Config::default(), // TODO make configurable
            vec![ApiTopic::Subscriptions],           // TODO make configurable
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

        // Start the processor
        let processor_senders = processor::spawn(config.processor, executor.clone());

        // Create the processor-adjacent managers
        let signature_collector =
            Arc::new(SignatureCollectorManager::new(processor_senders.clone()));
        let Ok(qbft_manager) =
            QbftManager::new(processor_senders.clone(), OperatorId(1), slot_clock.clone())
        else {
            return Err("Unable to initialize qbft manager".into());
        };

        let validator_store = Arc::new(AnchorValidatorStore::<_, E>::new(
            signature_collector,
            qbft_manager,
            slashing_protection,
            spec.clone(),
            genesis_validators_root,
            OperatorId(123),
        ));

        let duties_service = Arc::new(
            DutiesServiceBuilder::new()
                .slot_clock(slot_clock.clone())
                .beacon_nodes(beacon_nodes.clone())
                .validator_store(validator_store.clone())
                .spec(spec.clone())
                .executor(executor.clone())
                //.enable_high_validator_count_metrics(config.enable_high_validator_count_metrics)
                .distributed(true)
                .build()?,
        );

        // Update the metrics server.
        if let Some(ctx) = &http_metrics_shared_state {
            ctx.write().genesis_time = Some(genesis_time);
            //ctx.write().validator_store = Some(validator_store.clone());
            //ctx.write().duties_service = Some(duties_service.clone());
        }

        let mut block_service_builder = BlockServiceBuilder::new()
            .slot_clock(slot_clock.clone())
            .validator_store(validator_store.clone())
            .beacon_nodes(beacon_nodes.clone())
            .executor(executor.clone())
            .chain_spec(spec.clone());
        //.graffiti(config.graffiti)
        //.graffiti_file(config.graffiti_file.clone());

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
            //.builder_registration_timestamp_override(config.builder_registration_timestamp_override)
            .validator_registration_batch_size(500)
            .build()?;

        let sync_committee_service = SyncCommitteeService::new(
            duties_service.clone(),
            validator_store.clone(),
            slot_clock.clone(),
            beacon_nodes.clone(),
            executor.clone(),
        );

        // We use `SLOTS_PER_EPOCH` as the capacity of the block notification channel, because
        // we don't expect notifications to be delayed by more than a single slot, let alone a
        // whole epoch!
        let channel_capacity = E::slots_per_epoch() as usize;
        let (block_service_tx, block_service_rx) = mpsc::channel(channel_capacity);

        // Wait until genesis has occurred.
        wait_for_genesis(&beacon_nodes, genesis_time).await?;

        duties_service::start_update_service(duties_service.clone(), block_service_tx);

        block_service
            .start_update_service(block_service_rx)
            .map_err(|e| format!("Unable to start block service: {}", e))?;

        attestation_service
            .start_update_service(&spec)
            .map_err(|e| format!("Unable to start attestation service: {}", e))?;

        sync_committee_service
            .start_update_service(&spec)
            .map_err(|e| format!("Unable to start sync committee service: {}", e))?;

        preparation_service
            .start_update_service(&spec)
            .map_err(|e| format!("Unable to start preparation service: {}", e))?;

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
                    info!("Waiting for genesis",);
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

async fn wait_for_genesis(
    beacon_nodes: &BeaconNodeFallback<SystemTimeSlotClock>,
    genesis_time: u64,
) -> Result<(), String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("Unable to read system time: {:?}", e))?;
    let genesis_time = Duration::from_secs(genesis_time);

    // If the time now is less than (prior to) genesis, then delay until the
    // genesis instant.
    //
    // If the validator client starts before genesis, it will get errors from
    // the slot clock.
    if now < genesis_time {
        info!(
            seconds_to_wait = (genesis_time - now).as_secs(),
            "Starting node prior to genesis",
        );

        // Start polling the node for pre-genesis information, cancelling the polling as soon as the
        // timer runs out.
        tokio::select! {
            result = poll_whilst_waiting_for_genesis(beacon_nodes, genesis_time) => result?,
            () = sleep(genesis_time - now) => ()
        };

        info!(
            ms_since_genesis = (genesis_time - now).as_millis(),
            "Genesis has occurred",
        );
    } else {
        info!(
            seconds_ago = (now - genesis_time).as_secs(),
            "Genesis has already occurred",
        );
    }

    Ok(())
}

/// Request the version from the node, looping back and trying again on failure. Exit once the node
/// has been contacted.
async fn poll_whilst_waiting_for_genesis(
    beacon_nodes: &BeaconNodeFallback<SystemTimeSlotClock>,
    genesis_time: Duration,
) -> Result<(), String> {
    loop {
        match beacon_nodes
            .first_success(|beacon_node| async move { beacon_node.get_lighthouse_staking().await })
            .await
        {
            Ok(is_staking) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|e| format!("Unable to read system time: {:?}", e))?;

                if !is_staking {
                    error!(
                        msg = "this will caused missed duties",
                        info = "see the --staking CLI flag on the beacon node",
                        "Staking is disabled for beacon node"
                    );
                }

                if now < genesis_time {
                    info!(
                        bn_staking_enabled = is_staking,
                        seconds_to_wait = (genesis_time - now).as_secs(),
                        "Waiting for genesis"
                    );
                } else {
                    break Ok(());
                }
            }
            Err(e) => {
                error!(
                    error = ?e.0,
                    "Error polling beacon node",
                );
            }
        }

        sleep(WAITING_FOR_GENESIS_POLL_TIME).await;
    }
}

pub fn load_pem_certificate<P: AsRef<Path>>(pem_path: P) -> Result<Certificate, String> {
    let mut buf = Vec::new();
    File::open(&pem_path)
        .map_err(|e| format!("Unable to open certificate path: {}", e))?
        .read_to_end(&mut buf)
        .map_err(|e| format!("Unable to read certificate file: {}", e))?;
    Certificate::from_pem(&buf).map_err(|e| format!("Unable to parse certificate: {}", e))
}

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use beacon_node_fallback::BeaconNodeFallback;
use eth2::{BeaconNodeHttpClient, types::BlockId};
use futures::{StreamExt, stream::FuturesUnordered};
use slot_clock::SlotClock;
use ssv_types::{ValidatorIndex, consensus::BeaconVote};
use task_executor::TaskExecutor;
use tokio::time::sleep;
use tracing::{debug, error, info, trace, warn};
use types::{AttestationData, ChainSpec, EthSpec, Hash256, Slot};
use validator_services::duties_service::DutiesService;

use crate::{AnchorValidatorStore, ContributionWaiter, SlotMetadata};

const SOFT_TIMEOUT: Duration = Duration::from_secs(1);
const HARD_TIMEOUT: Duration = Duration::from_secs(3);
const BLOCK_SLOT_LOOKUP_TIMEOUT: Duration = Duration::from_millis(500);

pub struct MetadataService<E: EthSpec, T: SlotClock + 'static> {
    duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
    validator_store: Arc<AnchorValidatorStore<T, E>>,
    slot_clock: T,
    beacon_nodes: Arc<BeaconNodeFallback<T>>,
    executor: TaskExecutor,
    spec: Arc<ChainSpec>,
    weighted_attestation_data: bool,
}

impl<E: EthSpec, T: SlotClock + 'static> MetadataService<E, T> {
    pub fn new(
        duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
        validator_store: Arc<AnchorValidatorStore<T, E>>,
        slot_clock: T,
        beacon_nodes: Arc<BeaconNodeFallback<T>>,
        executor: TaskExecutor,
        spec: Arc<ChainSpec>,
        weighted_attestation_data: bool,
    ) -> Self {
        Self {
            duties_service,
            validator_store,
            slot_clock,
            beacon_nodes,
            executor,
            spec,
            weighted_attestation_data,
        }
    }

    pub fn start_update_service(self) -> Result<(), String> {
        let slot_duration = Duration::from_secs(self.spec.seconds_per_slot);
        let duration_to_next_slot = self
            .slot_clock
            .duration_to_next_slot()
            .ok_or("Unable to determine duration to next slot")?;

        info!(
            next_update_millis = duration_to_next_slot.as_millis(),
            "Metadata service started"
        );

        let executor = self.executor.clone();

        let interval_fut = async move {
            loop {
                if let Some(duration_to_next_slot) = self.slot_clock.duration_to_next_slot() {
                    sleep(duration_to_next_slot + slot_duration / 3).await;

                    if let Err(err) = self.update_metadata().await {
                        error!(err, "Failed to update slot metadata")
                    } else {
                        trace!("Updated slot metadata");
                    }
                } else {
                    error!("Failed to read slot clock");
                    // If we can't read the slot clock, just wait another slot.
                    sleep(slot_duration).await;
                }
            }
        };

        executor.spawn(interval_fut, "metadata_service");
        Ok(())
    }

    async fn update_metadata(&self) -> Result<(), String> {
        let slot = self.slot_clock.now().ok_or("Failed to read slot clock")?;

        let attestation_data = if self.weighted_attestation_data {
            self.weighted_calculation(slot).await?
        } else {
            self.beacon_nodes
                .first_success(|beacon_node| async move {
                    let _timer = validator_metrics::start_timer_vec(
                        &validator_metrics::ATTESTATION_SERVICE_TIMES,
                        &[validator_metrics::ATTESTATIONS_HTTP_GET],
                    );
                    beacon_node
                        .get_validator_attestation_data(slot, 0)
                        .await
                        .map_err(|e| format!("Failed to produce attestation data: {e:?}"))
                        .map(|result| result.data)
                })
                .await
                .map_err(|e| e.to_string())?
        };

        let beacon_vote = BeaconVote {
            block_root: attestation_data.beacon_block_root,
            source: attestation_data.source,
            target: attestation_data.target,
        };

        let (attesting_validator_indices, attesting_validator_committees) = self
            .duties_service
            .attesters(slot)
            .into_iter()
            .map(|duty| {
                (
                    ValidatorIndex(duty.duty.validator_index as usize),
                    (duty.duty.pubkey, duty.duty.committee_index),
                )
            })
            .unzip();

        let sync_duties = self
            .duties_service
            .sync_duties
            .get_duties_for_slot::<E>(slot, &self.spec);

        let sync_validators = sync_duties
            .as_ref()
            .map(|duties| {
                duties
                    .duties
                    .iter()
                    .map(|duty| ValidatorIndex(duty.validator_index as usize))
                    .collect()
            })
            .unwrap_or_default();

        let multi_sync_aggregators = sync_duties
            .map(|duties| {
                let mut aggregators_by_validator = HashMap::new();
                for (_, aggregators) in duties.aggregators {
                    for (_, pk, _) in aggregators {
                        *aggregators_by_validator.entry(pk).or_insert(0) += 1;
                    }
                }
                aggregators_by_validator
                    .into_iter()
                    .filter(|(_, count)| *count > 1)
                    .map(|(pk, count)| (pk, ContributionWaiter::new(count)))
                    .collect()
            })
            .unwrap_or_default();

        let metadata = SlotMetadata {
            slot,
            beacon_vote,
            attesting_validator_indices,
            attesting_validator_committees,
            sync_validators,
            multi_sync_aggregators,
        };

        self.validator_store.update_slot_metadata(metadata);

        Ok(())
    }

    async fn weighted_calculation(&self, slot: Slot) -> Result<AttestationData, String> {
        let started = Instant::now();

        let clients: Vec<(String, BeaconNodeHttpClient)> = {
            let candidates = self.beacon_nodes.candidates.read().await;
            candidates
                .iter()
                .map(|c| (c.beacon_node.to_string(), c.beacon_node.clone()))
                .collect()
        };

        let num_clients = clients.len();

        debug!(num_clients, "Starting weighted attestation data fetch");

        // Spawn all fetch requests in parallel
        let mut futures: FuturesUnordered<_> = clients
            .into_iter()
            .map(|(addr, client)| async move {
                let result = self.fetch_and_score(&client, slot).await;
                (addr, result)
            })
            .collect();

        let mut succeeded = 0;
        let mut failed = 0;
        let mut best_data: Option<ScoredAttestationData> = None;
        let mut soft_timeout_hit = false;

        // We have two timeouts: a soft timeout and a hard timeout.
        // At the soft timeout, we return if we have any responses so far.
        // At the hard timeout, we return unconditionally.
        // The soft timeout is half the duration of the hard timeout.

        // Collect responses until soft timeout (1s)
        let soft_timeout = sleep(SOFT_TIMEOUT);
        tokio::pin!(soft_timeout);

        loop {
            tokio::select! {
                biased;

                Some((addr, result)) = futures.next() => {
                    match result {
                        Ok(scored_attestation) => {
                            succeeded += 1;
                            trace!(
                                elapsed_ms = started.elapsed().as_millis(),
                                client = %scored_attestation.client_addr,
                                score = scored_attestation.score,
                                succeeded,
                                failed,
                                "Attestation data received"
                            );

                            // Update best if this score is higher
                            best_data = Some(match best_data {
                                Some(current) if current.score >= scored_attestation.score => current,
                                _ => {
                                    debug!(
                                        client = %scored_attestation.client_addr,
                                        score = scored_attestation.score,
                                        "New best attestation data"
                                    );
                                    scored_attestation
                                }
                            });
                        }
                        Err(e) => {
                            failed += 1;
                            warn!(
                                elapsed_ms = started.elapsed().as_millis(),
                                client = %addr,
                                error = %e,
                                succeeded,
                                failed,
                                "Failed to fetch attestation data"
                            );
                        }
                    }

                    // All responses received, exit early
                    if succeeded + failed == num_clients {
                        break;
                    }
                }

                () = &mut soft_timeout, if !soft_timeout_hit => {
                    soft_timeout_hit = true;
                    debug!(
                        elapsed_ms = started.elapsed().as_millis(),
                        succeeded,
                        failed,
                        pending = num_clients - succeeded - failed,
                        "Soft timeout reached"
                    );

                    // If we have at least one response, return early
                    if best_data.is_some() {
                        break;
                    }
                }

                else => break,
            }
        }

        // If no responses yet, wait until hard timeout (1s)
        if best_data.is_none() && succeeded + failed < num_clients {
            let remaining = HARD_TIMEOUT.saturating_sub(started.elapsed());
            let hard_timeout = sleep(remaining);
            tokio::pin!(hard_timeout);

            loop {
                tokio::select! {
                    biased;

                    Some((addr, result)) = futures.next() => {
                        match result {
                            Ok(scored_attestation) => {
                                succeeded += 1;
                                trace!(
                                    elapsed_ms = started.elapsed().as_millis(),
                                    client = %scored_attestation.client_addr,
                                    score = scored_attestation.score,
                                    "Response received (hard timeout phase)"
                                );

                                best_data = Some(match best_data {
                                    Some(current) if current.score >= scored_attestation.score => current,
                                    _ => scored_attestation,
                                });
                            }
                            Err(e) => {
                                failed += 1;
                                warn!(
                                    client = %addr,
                                    error = %e,
                                    "Error in hard timeout phase"
                                );
                            }
                        }

                        if succeeded + failed == num_clients {
                            break;
                        }
                    }

                    () = &mut hard_timeout => {
                        error!(
                            elapsed_ms = started.elapsed().as_millis(),
                            succeeded,
                            failed,
                            timed_out = num_clients - succeeded - failed,
                            "Hard timeout reached"
                        );
                        break;
                    }

                    else => break,
                }
            }
        }

        // Return best result or error if none received
        match best_data {
            Some(scored_attestation) => {
                debug!(
                    elapsed_ms = started.elapsed().as_millis(),
                    client = %scored_attestation.client_addr,
                    score = scored_attestation.score,
                    succeeded,
                    failed,
                    "Selected best attestation data"
                );
                Ok(scored_attestation.attestation_data)
            }
            None => Err(format!(
                "No attestation data received from any of {} beacon nodes (succeeded: {}, failed: {})",
                num_clients, succeeded, failed
            )),
        }
    }

    async fn fetch_and_score(
        &self,
        client: &BeaconNodeHttpClient,
        slot: Slot,
    ) -> Result<ScoredAttestationData, String> {
        let client_addr = client.to_string();

        // Get attestation data
        let attestation_data = client
            .get_validator_attestation_data(slot, 0)
            .await
            .map_err(|e| format!("{client_addr}: {e:?}"))?
            .data;

        // Calculate base score from checkpoint epochs (higher epochs = more recent)
        let base_score = (attestation_data.source.epoch.as_u64()
            + attestation_data.target.epoch.as_u64()) as f64;

        // Try to get head slot for bonus scoring
        let score = match self
            .get_block_slot(client, attestation_data.beacon_block_root)
            .await
        {
            Some(head_slot) => {
                let attestation_slot_u64 = slot.as_u64();
                let head_slot_u64 = head_slot.as_u64();

                if head_slot_u64 <= attestation_slot_u64 {
                    // Increase score based on the nearness of the head slot
                    let distance = attestation_slot_u64 - head_slot_u64;
                    let bonus = 1.0 / (1 + distance) as f64;

                    trace!(
                        client = %client_addr,
                        head_slot = head_slot_u64,
                        attestation_slot = attestation_slot_u64,
                        source_epoch = attestation_data.source.epoch.as_u64(),
                        target_epoch = attestation_data.target.epoch.as_u64(),
                        distance,
                        base_score,
                        bonus,
                        total_score = base_score + bonus,
                        "Scored attestation data"
                    );

                    base_score + bonus
                } else {
                    warn!(
                        client = %client_addr,
                        head_slot = head_slot_u64,
                        attestation_slot = attestation_slot_u64,
                        "Block slot is the same or after attestation slot, skipping proximity bonus"
                    );
                    base_score
                }
            }
            None => {
                trace!(
                    client = %client_addr,
                    base_score,
                    "Using base score only (no head slot)"
                );
                base_score
            }
        };

        Ok(ScoredAttestationData {
            client_addr,
            attestation_data,
            score,
        })
    }

    /// Get the slot number for a given block root with timeout
    async fn get_block_slot(
        &self,
        client: &BeaconNodeHttpClient,
        block_root: Hash256,
    ) -> Option<Slot> {
        tokio::time::timeout(BLOCK_SLOT_LOOKUP_TIMEOUT, async {
            client
                .get_beacon_headers_block_id(BlockId::Root(block_root))
                .await
                .ok()
                .flatten()
                .map(|resp| resp.data.header.message.slot)
        })
        .await
        .ok()
        .flatten()
    }
}

#[derive(Debug, Clone)]
struct ScoredAttestationData {
    client_addr: String,
    attestation_data: AttestationData,
    score: f64,
}

#[cfg(test)]
mod tests;

//! Shamelessly stolen from lighthouse and adapted for anchor purposes:
//! - Provide attestation data when requesting signature to ensure valid `BeaconVote` for qbft
//! - One aggregation request per validator including every subnet

use crate::{AnchorValidatorStore, ContributionAndProofSigningData};
use beacon_node_fallback::{ApiTopic, BeaconNodeFallback};
use futures::future::join_all;
use futures::future::FutureExt;
use slot_clock::SlotClock;
use ssv_types::message::BeaconVote;
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use task_executor::TaskExecutor;
use tokio::time::{sleep, sleep_until, Duration, Instant};
use tracing::{debug, error, info, trace};
use types::{
    ChainSpec, EthSpec, Hash256, PublicKeyBytes, Slot, SyncCommitteeSubscription,
    SyncContributionData, SyncDuty, SyncSelectionProof, SyncSubnetId,
};
use validator_services::duties_service::DutiesService;
use validator_store::Error as ValidatorStoreError;

pub const SUBSCRIPTION_LOOKAHEAD_EPOCHS: u64 = 4;

pub struct SyncCommitteeService<T: SlotClock + 'static, E: EthSpec> {
    inner: Arc<Inner<T, E>>,
}

impl<T: SlotClock + 'static, E: EthSpec> Clone for SyncCommitteeService<T, E> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: SlotClock + 'static, E: EthSpec> Deref for SyncCommitteeService<T, E> {
    type Target = Inner<T, E>;

    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

pub struct Inner<T: SlotClock + 'static, E: EthSpec> {
    duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
    validator_store: Arc<AnchorValidatorStore<T, E>>,
    slot_clock: T,
    beacon_nodes: Arc<BeaconNodeFallback<T>>,
    executor: TaskExecutor,
    /// Boolean to track whether the service has posted subscriptions to the BN at least once.
    ///
    /// This acts as a latch that fires once upon start-up, and then never again.
    first_subscription_done: AtomicBool,
}

impl<T: SlotClock + 'static, E: EthSpec> SyncCommitteeService<T, E> {
    pub fn new(
        duties_service: Arc<DutiesService<AnchorValidatorStore<T, E>, T>>,
        validator_store: Arc<AnchorValidatorStore<T, E>>,
        slot_clock: T,
        beacon_nodes: Arc<BeaconNodeFallback<T>>,
        executor: TaskExecutor,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                duties_service,
                validator_store,
                slot_clock,
                beacon_nodes,
                executor,
                first_subscription_done: AtomicBool::new(false),
            }),
        }
    }

    /// Check if the Altair fork has been activated and therefore sync duties should be performed.
    ///
    /// Slot clock errors are mapped to `false`.
    fn altair_fork_activated(&self) -> bool {
        self.duties_service
            .spec
            .altair_fork_epoch
            .and_then(|fork_epoch| {
                let current_epoch = self.slot_clock.now()?.epoch(E::slots_per_epoch());
                Some(current_epoch >= fork_epoch)
            })
            .unwrap_or(false)
    }

    pub fn start_update_service(self, spec: &ChainSpec) -> Result<(), String> {
        let slot_duration = Duration::from_secs(spec.seconds_per_slot);
        let duration_to_next_slot = self
            .slot_clock
            .duration_to_next_slot()
            .ok_or("Unable to determine duration to next slot")?;

        info!(
            next_update_millis = duration_to_next_slot.as_millis(),
            "Sync committee service started"
        );

        let executor = self.executor.clone();

        let interval_fut = async move {
            loop {
                if let Some(duration_to_next_slot) = self.slot_clock.duration_to_next_slot() {
                    // Wait for contribution broadcast interval 1/3 of the way through the slot.
                    sleep(duration_to_next_slot + slot_duration / 3).await;

                    // Do nothing if the Altair fork has not yet occurred.
                    if !self.altair_fork_activated() {
                        continue;
                    }

                    if let Err(e) = self.spawn_contribution_tasks(slot_duration).await {
                        error!(
                            error = ?e,
                            "Failed to spawn sync contribution tasks"
                        )
                    } else {
                        trace!("Spawned sync contribution tasks")
                    }

                    // Do subscriptions for future slots/epochs.
                    self.spawn_subscription_tasks();
                } else {
                    error!("Failed to read slot clock");
                    // If we can't read the slot clock, just wait another slot.
                    sleep(slot_duration).await;
                }
            }
        };

        executor.spawn(interval_fut, "sync_committee_service");
        Ok(())
    }

    async fn spawn_contribution_tasks(&self, slot_duration: Duration) -> Result<(), String> {
        let slot = self.slot_clock.now().ok_or("Failed to read slot clock")?;
        let duration_to_next_slot = self
            .slot_clock
            .duration_to_next_slot()
            .ok_or("Unable to determine duration to next slot")?;

        // If a validator needs to publish a sync aggregate, they must do so at 2/3
        // through the slot. This delay triggers at this time
        let aggregate_production_instant = Instant::now()
            + duration_to_next_slot
                .checked_sub(slot_duration / 3)
                .unwrap_or_else(|| Duration::from_secs(0));

        let Some(slot_duties) = self
            .duties_service
            .sync_duties
            .get_duties_for_slot::<E>(slot, &self.duties_service.spec)
        else {
            debug!("No duties known for slot {}", slot);
            return Ok(());
        };

        if slot_duties.duties.is_empty() {
            debug!(%slot, "No local validators in current sync committee");
            return Ok(());
        }

        // ANCHOR SPECIFIC - fetch source and target to enable proper qbft voting
        let attestation_data = self
            .beacon_nodes
            .first_success(|beacon_node| async move {
                let _timer = validator_metrics::start_timer_vec(
                    &validator_metrics::ATTESTATION_SERVICE_TIMES,
                    &[validator_metrics::ATTESTATIONS_HTTP_GET],
                );
                beacon_node
                    .get_validator_attestation_data(slot, 0)
                    .await
                    .map_err(|e| format!("Failed to produce attestation data: {:?}", e))
                    .map(|result| result.data)
            })
            .await
            .map_err(|e| e.to_string())?;

        let block_root = attestation_data.beacon_block_root;
        let beacon_vote = BeaconVote {
            block_root,
            source: attestation_data.source,
            target: attestation_data.target,
        };

        // Spawn one task to publish all of the sync committee signatures.
        let validator_duties = slot_duties.duties;
        let service = self.clone();
        self.inner.executor.spawn(
            async move {
                service
                    .publish_sync_committee_signatures(slot, beacon_vote, validator_duties)
                    .map(|_| ())
                    .await
            },
            "sync_committee_signature_publish",
        );

        let aggregators = slot_duties.aggregators;
        let service = self.clone();
        self.inner.executor.spawn(
            async move {
                service
                    .publish_sync_committee_aggregates(
                        slot,
                        block_root,
                        aggregators,
                        aggregate_production_instant,
                    )
                    .map(|_| ())
                    .await
            },
            "sync_committee_aggregate_publish",
        );

        Ok(())
    }

    /// Publish sync committee signatures.
    async fn publish_sync_committee_signatures(
        &self,
        slot: Slot,
        vote: BeaconVote,
        validator_duties: Vec<SyncDuty>,
    ) -> Result<(), ()> {
        // Create futures to produce sync committee signatures.
        let signature_futures = validator_duties.iter().map(|duty| {
            let vote = vote.clone();
            async move {
                match self
                    .validator_store
                    .produce_sync_committee_signature_with_full_vote(
                        slot,
                        vote.clone(),
                        duty.validator_index,
                        &duty.pubkey,
                    )
                    .await
                {
                    Ok(signature) => Some(signature),
                    Err(ValidatorStoreError::UnknownPubkey(pubkey)) => {
                        // A pubkey can be missing when a validator was recently
                        // removed via the API.
                        debug!(
                            ?pubkey,
                            validator_index = duty.validator_index,
                            %slot,
                            "Missing pubkey for sync committee signature"
                        );
                        None
                    }
                    Err(e) => {
                        error!(
                            validator_index = duty.validator_index,
                            %slot,
                            error = ?e,
                            "Failed to sign sync committee signature"
                        );
                        None
                    }
                }
            }
        });

        // Execute all the futures in parallel, collecting any successful results.
        let committee_signatures = &join_all(signature_futures)
            .await
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

        self.beacon_nodes
            .request(ApiTopic::SyncCommittee, |beacon_node| async move {
                beacon_node
                    .post_beacon_pool_sync_committee_signatures(committee_signatures)
                    .await
            })
            .await
            .map_err(|e| {
                error!(
                    %slot,
                    error = %e,
                    "Unable to publish sync committee messages"
                );
            })?;

        info!(
            count = committee_signatures.len(),
            head_block = ?vote.block_root,
            %slot,
            "Successfully published sync committee messages"
        );

        Ok(())
    }

    async fn publish_sync_committee_aggregates(
        &self,
        slot: Slot,
        beacon_block_root: Hash256,
        aggregators: HashMap<SyncSubnetId, Vec<(u64, PublicKeyBytes, SyncSelectionProof)>>,
        aggregate_instant: Instant,
    ) {
        sleep_until(aggregate_instant).await;

        let contributions = aggregators.keys().map(|&subnet_id| {
            self.beacon_nodes
                .first_success(move |beacon_node| async move {
                    let sync_contribution_data = SyncContributionData {
                        slot,
                        beacon_block_root,
                        subcommittee_index: *subnet_id,
                    };

                    beacon_node
                        .get_validator_sync_committee_contribution(&sync_contribution_data)
                        .await
                        .map(|data| data.map(|data| (subnet_id, data.data)))
                })
        });

        let contributions = join_all(contributions)
            .await
            .into_iter()
            .filter_map(|result| match result {
                Ok(Some((subnet_id, data))) => Some((*subnet_id, data)),
                Ok(None) => {
                    error!(%slot, ?beacon_block_root, "No aggregate contribution found");
                    None
                }
                Err(err) => {
                    error!(
                        %slot,
                        ?beacon_block_root,
                        error = %err,
                        "Failed to produce sync contribution"
                    );
                    None
                }
            })
            .collect::<HashMap<_, _>>();

        let mut aggregators_by_validator = HashMap::new();
        for (subnet, aggregators) in aggregators {
            for aggregator in aggregators {
                if let Some(contribution) = contributions.get(&subnet).cloned() {
                    aggregators_by_validator
                        .entry((aggregator.0, aggregator.1))
                        .or_insert(vec![])
                        .push(ContributionAndProofSigningData {
                            contribution,
                            selection_proof: aggregator.2,
                        });
                }
            }
        }

        // Create futures to produce signed contributions.
        let signature_futures = aggregators_by_validator.into_iter().map(
            |((aggregator_index, aggregator_pk), data)| {
                self.validator_store.produce_signed_contribution_and_proofs(
                    aggregator_index,
                    aggregator_pk,
                    data,
                )
            },
        );

        // Execute all the futures in parallel, collecting any successful results.
        let signed_contributions = &join_all(signature_futures)
            .await
            .into_iter()
            .flatten()
            .filter_map(|result| match result {
                Ok(signed_contribution) => Some(signed_contribution),
                Err(ValidatorStoreError::UnknownPubkey(pubkey)) => {
                    // A pubkey can be missing when a validator was recently
                    // removed via the API.
                    debug!(?pubkey, %slot, "Missing pubkey for sync contribution");
                    None
                }
                Err(e) => {
                    error!(
                        %slot,
                        error = ?e,
                        "Unable to sign sync committee contribution"
                    );
                    None
                }
            })
            .collect::<Vec<_>>();

        // Publish to the beacon node.
        if let Err(err) = self
            .beacon_nodes
            .first_success(|beacon_node| async move {
                beacon_node
                    .post_validator_contribution_and_proofs(signed_contributions)
                    .await
            })
            .await
        {
            error!(
                %slot,
                error = %err,
                "Unable to publish signed contributions and proofs"
            );
        }

        info!(
            beacon_block_root = %beacon_block_root,
            %slot,
            "Successfully published sync contributions"
        );
    }

    fn spawn_subscription_tasks(&self) {
        let service = self.clone();

        self.inner.executor.spawn(
            async move {
                service.publish_subscriptions().await.unwrap_or_else(|e| {
                    error!(
                        error = ?e,
                        "Error publishing subscriptions"
                    )
                });
            },
            "sync_committee_subscription_publish",
        );
    }

    async fn publish_subscriptions(self) -> Result<(), String> {
        let spec = &self.duties_service.spec;
        let slot = self.slot_clock.now().ok_or("Failed to read slot clock")?;

        let mut duty_slots = vec![];
        let mut all_succeeded = true;

        // At the start of every epoch during the current period, re-post the subscriptions
        // to the beacon node. This covers the case where the BN has forgotten the subscriptions
        // due to a restart, or where the VC has switched to a fallback BN.
        let current_period = sync_period_of_slot::<E>(slot, spec)?;

        if !self.first_subscription_done.load(Ordering::Relaxed)
            || slot.as_u64() % E::slots_per_epoch() == 0
        {
            duty_slots.push((slot, current_period));
        }

        // Near the end of the current period, push subscriptions for the next period to the
        // beacon node. We aggressively push every slot in the lead-up, as this is the main way
        // that we want to ensure that the BN is subscribed (well in advance).
        let lookahead_slot = slot + SUBSCRIPTION_LOOKAHEAD_EPOCHS * E::slots_per_epoch();

        let lookahead_period = sync_period_of_slot::<E>(lookahead_slot, spec)?;

        if lookahead_period > current_period {
            duty_slots.push((lookahead_slot, lookahead_period));
        }

        if duty_slots.is_empty() {
            return Ok(());
        }

        // Collect subscriptions.
        let mut subscriptions = vec![];

        for (duty_slot, sync_committee_period) in duty_slots {
            debug!(%duty_slot, %slot, "Fetching subscription duties");
            match self
                .duties_service
                .sync_duties
                .get_duties_for_slot::<E>(duty_slot, spec)
            {
                Some(duties) => subscriptions.extend(subscriptions_from_sync_duties(
                    duties.duties,
                    sync_committee_period,
                    spec,
                )),
                None => {
                    debug!(
                        slot = %duty_slot,
                        "No duties for subscription"
                    );
                    all_succeeded = false;
                }
            }
        }

        if subscriptions.is_empty() {
            debug!(%slot, "No sync subscriptions to send");
            return Ok(());
        }

        // Post subscriptions to BN.
        debug!(
            count = subscriptions.len(),
            "Posting sync subscriptions to BN"
        );
        let subscriptions_slice = &subscriptions;

        for subscription in subscriptions_slice {
            debug!(
                validator_index = subscription.validator_index,
                validator_sync_committee_indices = ?subscription.sync_committee_indices,
                until_epoch = %subscription.until_epoch,
                "Subscription"
            );
        }

        if let Err(e) = self
            .beacon_nodes
            .request(ApiTopic::Subscriptions, |beacon_node| async move {
                beacon_node
                    .post_validator_sync_committee_subscriptions(subscriptions_slice)
                    .await
            })
            .await
        {
            error!(
                %slot,
                error = %e,
                "Unable to post sync committee subscriptions"
            );
            all_succeeded = false;
        }

        // Disable first-subscription latch once all duties have succeeded once.
        if all_succeeded {
            self.first_subscription_done.store(true, Ordering::Relaxed);
        }

        Ok(())
    }
}

fn sync_period_of_slot<E: EthSpec>(slot: Slot, spec: &ChainSpec) -> Result<u64, String> {
    slot.epoch(E::slots_per_epoch())
        .sync_committee_period(spec)
        .map_err(|e| format!("Error computing sync period: {:?}", e))
}

fn subscriptions_from_sync_duties(
    duties: Vec<SyncDuty>,
    sync_committee_period: u64,
    spec: &ChainSpec,
) -> impl Iterator<Item = SyncCommitteeSubscription> {
    let until_epoch = spec.epochs_per_sync_committee_period * (sync_committee_period + 1);
    duties
        .into_iter()
        .map(move |duty| SyncCommitteeSubscription {
            validator_index: duty.validator_index,
            sync_committee_indices: duty.validator_sync_committee_indices,
            until_epoch,
        })
}

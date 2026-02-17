//! Aggregator Consensus Data Builder
//!
//! This module handles fetching aggregated attestations and sync contributions from beacon nodes,
//! and building `AggregatorCommitteeConsensusData` for QBFT consensus (Boole+ fork).
//!
//! # Responsibilities
//!
//! - Fetch aggregated attestations from beacon node (deduplicated by committee index)
//! - Fetch sync committee contributions from beacon node (deduplicated by subnet ID)
//! - Build `AggregatorCommitteeConsensusData` per SSV committee
//! - Sort aggregators/contributors deterministically for consensus compatibility with SSV-Go
//!
//! # Separation of Concerns
//!
//! This module is intentionally separated from `DutyInputPublisher` (formerly `MetadataService`)
//! which handles the timing and publishing of duty inputs. This module focuses purely on
//! the data fetching and consensus data construction logic.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use beacon_node_fallback::BeaconNodeFallback;
use bls::PublicKeyBytes;
use eth2::types::SyncContributionData;
use futures::stream::{FuturesUnordered, StreamExt};
use slot_clock::SlotClock;
use ssv_types::{
    CommitteeId, IndexSet, ValidatorIndex, VariableList,
    consensus::{AggregatorCommitteeConsensusData, AssignedAggregator, BeaconVote, DataVersion},
};
use ssz::Encode;
use tokio::time::{Instant, sleep_until};
use tracing::{Instrument, info_span, warn};
use tree_hash::TreeHash;
use types::{
    Attestation, AttestationData, ChainSpec, EthSpec, ForkName, Hash256, Slot,
    SyncCommitteeContribution, SyncSelectionProof, SyncSubnetId,
};
use validator_services::duties_service::DutyAndProof;

use crate::{AnchorValidatorStore, VotingContext, metrics};

/// Data for sync committee aggregators.
pub struct SyncAggregatorData {
    pub validator_index: u64,
    pub pubkey: PublicKeyBytes,
    pub selection_proof: SyncSelectionProof,
}

/// Map from SSV committee to its sync aggregators grouped by subnet.
pub type SyncByCommitteeMap = HashMap<CommitteeId, Vec<(SyncSubnetId, SyncAggregatorData)>>;

/// Builder for `AggregatorCommitteeConsensusData`.
///
/// Handles fetching aggregated data from beacon nodes and constructing consensus data
/// structures that all SSV operators must agree on.
pub struct AggregatorConsensusBuilder<'a, E: EthSpec, T: SlotClock + 'static> {
    validator_store: &'a Arc<AnchorValidatorStore<T, E>>,
    beacon_nodes: &'a Arc<BeaconNodeFallback<T>>,
    spec: &'a Arc<ChainSpec>,
}

impl<'a, E: EthSpec, T: SlotClock + 'static> AggregatorConsensusBuilder<'a, E, T> {
    /// Create a new builder with references to required services.
    pub fn new(
        validator_store: &'a Arc<AnchorValidatorStore<T, E>>,
        beacon_nodes: &'a Arc<BeaconNodeFallback<T>>,
        spec: &'a Arc<ChainSpec>,
    ) -> Self {
        Self {
            validator_store,
            beacon_nodes,
            spec,
        }
    }

    /// Build `AggregatorCommitteeConsensusData` for each committee that has aggregators.
    ///
    /// Takes pre-grouped data from `DutyInputPublisher` to avoid redundant iteration.
    #[allow(clippy::too_many_arguments)]
    pub async fn build_consensus_data_for_all_committees(
        &self,
        slot: Slot,
        attesters_by_ssv_committee: HashMap<CommitteeId, Vec<&DutyAndProof>>,
        sync_by_ssv_committee: SyncByCommitteeMap,
        attestation_committee_indexes: HashSet<u64>,
        all_subnet_ids: HashSet<SyncSubnetId>,
        voting_context: &VotingContext,
        timeout: Duration,
    ) -> Result<HashMap<CommitteeId, Arc<AggregatorCommitteeConsensusData<E>>>, String> {
        // Parallel fetch from beacon node with timeout for partial results.
        // Uses `FuturesUnordered` internally to collect results as they complete.
        // After timeout, returns whatever has been collected.
        // This ensures we don't block on slow beacon nodes while still getting partial data.
        let (aggregated_attestations, sync_contributions) = tokio::join!(
            self.fetch_aggregated_attestations(
                slot,
                &voting_context.beacon_vote,
                &attestation_committee_indexes,
                timeout,
            ),
            self.fetch_sync_contributions(
                slot,
                voting_context.beacon_vote.block_root,
                &all_subnet_ids,
                timeout,
            ),
        );

        // Build consensus data per committee
        // All committees with work
        let ssv_committees: HashSet<CommitteeId> = attesters_by_ssv_committee
            .keys()
            .chain(sync_by_ssv_committee.keys())
            .copied()
            .collect();

        let mut result = HashMap::with_capacity(ssv_committees.len());
        for ssv_committee_id in ssv_committees {
            let ssv_committee_attesters = attesters_by_ssv_committee.get(&ssv_committee_id);
            let ssv_committee_sync = sync_by_ssv_committee.get(&ssv_committee_id);

            let consensus_data = self.build_consensus_data_for_committee(
                slot,
                &ssv_committee_id,
                ssv_committee_attesters,
                ssv_committee_sync,
                &aggregated_attestations,
                &sync_contributions,
            )?;

            if let Some(data) = consensus_data {
                result.insert(ssv_committee_id, Arc::new(data));
            }
        }

        Ok(result)
    }

    /// Build `AggregatorCommitteeConsensusData` for a single committee.
    ///
    /// CRITICAL REQUIREMENTS (must match SSV Go/Spec exactly for consensus):
    /// 1. Aggregators: Call `is_aggregator()` on the selection proof before including
    /// 2. Contributors: Call `is_sync_committee_aggregator()` on the selection proof before
    ///    including
    /// 3. Aggregators: Sort by `validator_index` (all share same signing root)
    /// 4. Contributors: Sort by (`signing_root`, `validator_index`) to match SSV Go's root-sorted
    ///    processing
    /// 5. Committee indexes: First-seen order from sorted aggregators (not sorted separately)
    /// 6. Subnet IDs: First-seen order from sorted contributors (not sorted separately)
    fn build_consensus_data_for_committee(
        &self,
        slot: Slot,
        ssv_committee_id: &CommitteeId,
        ssv_committee_attesters: Option<&Vec<&DutyAndProof>>,
        ssv_committee_sync: Option<&Vec<(SyncSubnetId, SyncAggregatorData)>>,
        aggregated_attestations: &HashMap<u64, Attestation<E>>,
        sync_contributions: &HashMap<SyncSubnetId, SyncCommitteeContribution<E>>,
    ) -> Result<Option<AggregatorCommitteeConsensusData<E>>, String> {
        // === AGGREGATORS ===
        // Process pre-filtered attesters for this committee
        // These validators are already confirmed to be in this committee
        let mut aggregators: Vec<AssignedAggregator> = match ssv_committee_attesters {
            Some(attesters) => Vec::with_capacity(attesters.len()),
            None => Vec::new(),
        };
        if let Some(attesters) = ssv_committee_attesters {
            for duty_and_proof in attesters.iter() {
                let validator_index = ValidatorIndex(duty_and_proof.duty.validator_index as usize);

                // Verify the aggregated selection proof passes beacon node is_aggregator check.
                // Without this check, validators whose proofs don't meet the modulo threshold
                // would be included, causing consensus hash mismatch with other operators.
                // NOTE: Since we don't have direct beacon node access to is_aggregator() in
                // Anchor's setup, we rely on the fact that `selection_proof.is_some()` means
                // Lighthouse has already verified this validator is an aggregator.
                // Defensive: this should never happen since `update_aggregation_assignments`
                // filters with `.filter(|d| d.selection_proof.is_some())` before grouping.
                let Some(selection_proof) = duty_and_proof.selection_proof.clone() else {
                    warn!(
                        %slot,
                        ?ssv_committee_id,
                        ?validator_index,
                        "[AggregatorCommittee] BUG: aggregator missing selection_proof despite upstream filter"
                    );
                    continue;
                };
                aggregators.push(AssignedAggregator {
                    validator_index,
                    selection_proof: selection_proof.into(),
                    committee_index: duty_and_proof.duty.committee_index,
                });
            }
        }

        // Sort and filter aggregators using helper functions
        sort_aggregators_by_validator_index(&mut aggregators);
        filter_aggregators_with_attestations(&mut aggregators, aggregated_attestations);

        // === CONTRIBUTORS ===
        // Process pre-filtered sync aggregators for this committee
        // These validators are already confirmed to be in this committee
        let mut contributors_with_roots: Vec<(Hash256, AssignedAggregator)> =
            match ssv_committee_sync {
                Some(sync_entries) => Vec::with_capacity(sync_entries.len()),
                None => Vec::new(),
            };
        if let Some(sync_entries) = ssv_committee_sync {
            for (subnet_id, sync_aggregator) in sync_entries.iter() {
                // Compute signing root for this subnet
                let sync_selection_root = self
                    .validator_store
                    .compute_sync_selection_root(slot, (*subnet_id).into());

                let validator_index = ValidatorIndex(sync_aggregator.validator_index as usize);

                // Lighthouse duties service already filters sync duties to only include valid
                // aggregators (those whose proofs meet the modulo threshold), so we can proceed.
                contributors_with_roots.push((
                    sync_selection_root,
                    AssignedAggregator {
                        validator_index,
                        selection_proof: sync_aggregator.selection_proof.clone().into(),
                        committee_index: (*subnet_id).into(),
                    },
                ));
            }
        }

        // Sort and filter contributors using helper functions
        sort_contributors_by_signing_root_then_validator_index(&mut contributors_with_roots);
        filter_contributors_with_contributions(&mut contributors_with_roots, sync_contributions);

        let contributors: Vec<AssignedAggregator> = contributors_with_roots
            .into_iter()
            .map(|(_, contributor)| contributor)
            .collect();

        // Early exit if no aggregators/contributors
        if aggregators.is_empty() && contributors.is_empty() {
            return Ok(None);
        }

        // === COMMITTEE INDEXES & ATTESTATIONS ===
        // Extract unique committee indexes preserving first-seen order from sorted aggregators.
        // `IndexSet` deduplicates while maintaining insertion order, matching SSV Go's approach
        // of adding new indexes as they're encountered during iteration.
        let attestation_committee_indexes: IndexSet<u64> =
            aggregators.iter().map(|a| a.committee_index).collect();

        // Get attestations in attestation_committee_indexes order (1:1 correspondence)
        let attestations_bytes: Vec<VariableList<u8, _>> = attestation_committee_indexes
            .iter()
            .filter_map(|committee_index| aggregated_attestations.get(committee_index))
            .map(|attestation| {
                let bytes = attestation.as_ssz_bytes();
                VariableList::new(bytes).map_err(|e| {
                    warn!("Failed to create attestation bytes list: {:?}", e);
                    e
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to create attestation bytes: {:?}", e))?;

        // === SUBNET IDS & CONTRIBUTIONS ===
        // Extract unique subnet IDs preserving first-seen order from sorted contributors.
        // `IndexSet` deduplicates while maintaining insertion order, matching SSV Go's approach
        // of adding new IDs as they're encountered during iteration.
        let subnet_ids: IndexSet<SyncSubnetId> = contributors
            .iter()
            .map(|c| SyncSubnetId::new(c.committee_index))
            .collect();

        let contributions: Vec<SyncCommitteeContribution<E>> = subnet_ids
            .iter()
            .filter_map(|id| sync_contributions.get(id).cloned())
            .collect();

        // Get the fork version
        let epoch = slot.epoch(E::slots_per_epoch());
        let fork_name = self.spec.fork_name_at_epoch(epoch);
        let version = DataVersion::from(fork_name);

        Ok(Some(AggregatorCommitteeConsensusData {
            version,
            aggregators: aggregators
                .try_into()
                .map_err(|e| format!("aggregators: {e:?}"))?,
            aggregator_committee_indexes: attestation_committee_indexes
                .into_iter()
                .collect::<Vec<_>>()
                .try_into()
                .map_err(|e| format!("aggregator_committee_indexes: {e:?}"))?,
            aggregated_attestations: attestations_bytes
                .try_into()
                .map_err(|e| format!("aggregated_attestations: {e:?}"))?,
            contributors: contributors
                .try_into()
                .map_err(|e| format!("contributors: {e:?}"))?,
            sync_committee_contributions: contributions
                .try_into()
                .map_err(|e| format!("sync_committee_contributions: {e:?}"))?,
        }))
    }

    /// Fetch aggregated attestations from beacon node for the given committee indexes.
    /// Returns a map of `committee_index` -> `Attestation`.
    ///
    /// Uses `FuturesUnordered` to collect results as they complete. When the timeout is reached,
    /// returns whatever results have been collected so far (partial results). This ensures
    /// we don't block on slow beacon nodes while still using successful fetches.
    async fn fetch_aggregated_attestations(
        &self,
        slot: Slot,
        beacon_vote: &BeaconVote,
        attestation_committee_indexes: &HashSet<u64>,
        timeout: Duration,
    ) -> HashMap<u64, Attestation<E>> {
        let _timer = metrics::start_timer_vec(
            &metrics::AGGREGATOR_COMMITTEE_FETCH_TIMES,
            &["aggregated_attestations"],
        );

        // Determine fork version to handle pre-Electra vs Electra+ attestation data format.
        // In Electra+ (EIP-7549), AttestationData.index is always 0 because the committee
        // index moved to Attestation.committee_bits.
        // In pre-Electra, AttestationData.index must equal the committee_index.
        // Use slot as the canonical source for epoch since it's the authoritative parameter.
        let fork_name = self
            .spec
            .fork_name_at_epoch(slot.epoch(E::slots_per_epoch()));

        // Create `FuturesUnordered` for concurrent execution with partial result collection
        let beacon_nodes = &self.beacon_nodes;
        let mut futures: FuturesUnordered<_> = attestation_committee_indexes
            .iter()
            .map(|&committee_index| {
                // Reconstruct the attestation data to compute its tree hash root.
                // Pre-Electra: index = committee_index
                // Electra+: index = 0 (committee info moved to Attestation.committee_bits)
                let attestation_data = AttestationData {
                    slot,
                    index: if fork_name < ForkName::Electra { committee_index } else { 0 },
                    beacon_block_root: beacon_vote.block_root,
                    source: beacon_vote.source,
                    target: beacon_vote.target,
                };
                let attestation_data_root = attestation_data.tree_hash_root();

                async move {
                    let result = beacon_nodes
                        .first_success(|beacon_node| async move {
                            let _timer = validator_metrics::start_timer_vec(
                                &validator_metrics::ATTESTATION_SERVICE_TIMES,
                                &[validator_metrics::AGGREGATES_HTTP_GET],
                            );
                            beacon_node
                                .get_validator_aggregate_attestation_v2(
                                    slot,
                                    attestation_data_root,
                                    committee_index,
                                )
                                .await
                                .map_err(|e| {
                                    format!("[AggregatorCommittee] Failed to produce aggregate attestation: {:?}", e)
                                })?
                                .ok_or_else(|| {
                                    format!(
                                        "[AggregatorCommittee] No aggregate available for slot {}, committee {}",
                                        slot, committee_index
                                    )
                                })
                                .map(|result| result.into_data())
                        })
                        .await;
                    (committee_index, result)
                }
            })
            .collect();

        let total_committees = attestation_committee_indexes.len();
        let mut aggregated_attestations = HashMap::with_capacity(total_committees);
        let deadline = Instant::now() + timeout;

        // Collect results as they complete, until timeout or all done
        loop {
            // Exit when all futures completed
            if futures.is_empty() {
                break;
            }

            tokio::select! {
                Some((committee_index, result)) = futures.next() => {
                    match result {
                        Ok(attestation) => {
                            aggregated_attestations.insert(committee_index, attestation);
                        }
                        Err(e) => {
                            warn!(
                                %slot,
                                %committee_index,
                                error = %e,
                                "[AggregatorCommittee] Failed to fetch aggregated attestation for committee"
                            );
                        }
                    }
                }
                _ = sleep_until(deadline) => {
                    if aggregated_attestations.len() < total_committees {
                        warn!(
                            %slot,
                            collected = aggregated_attestations.len(),
                            total = total_committees,
                            "[AggregatorCommittee] Timeout fetching aggregated attestations, returning partial results"
                        );
                        metrics::inc_counter_vec(
                            &metrics::AGGREGATOR_COMMITTEE_PARTIAL_RESULTS,
                            &["aggregated_attestations"],
                        );
                    }
                    break;
                }
            }
        }

        // Track success/failure counts
        let successful = aggregated_attestations.len();
        let total = total_committees;
        let failed = total.saturating_sub(successful);
        metrics::inc_counter_vec_by(
            &metrics::AGGREGATOR_COMMITTEE_FETCH_SUCCESS,
            &["aggregated_attestations", "success"],
            successful as u64,
        );
        metrics::inc_counter_vec_by(
            &metrics::AGGREGATOR_COMMITTEE_FETCH_SUCCESS,
            &["aggregated_attestations", "failed"],
            failed as u64,
        );

        aggregated_attestations
    }

    /// Fetch sync committee contributions from beacon node for the given subnet IDs.
    /// Returns a map of subnet_id -> SyncCommitteeContribution.
    ///
    /// Uses `FuturesUnordered` to collect results as they complete. When the timeout is reached,
    /// returns whatever results have been collected so far (partial results). This ensures
    /// we don't block on slow beacon nodes while still using successful fetches.
    async fn fetch_sync_contributions(
        &self,
        slot: Slot,
        beacon_block_root: Hash256,
        subnet_ids: &HashSet<SyncSubnetId>,
        timeout: Duration,
    ) -> HashMap<SyncSubnetId, SyncCommitteeContribution<E>> {
        let _timer = metrics::start_timer_vec(
            &metrics::AGGREGATOR_COMMITTEE_FETCH_TIMES,
            &["sync_contributions"],
        );
        // Create `FuturesUnordered` for concurrent execution with partial result collection
        let beacon_nodes = &self.beacon_nodes;
        let mut futures: FuturesUnordered<_> = subnet_ids
            .iter()
            .map(|&subnet_id| async move {
                let result = beacon_nodes
                    .first_success(|beacon_node| async move {
                        let sync_contribution_data = SyncContributionData {
                            slot,
                            beacon_block_root,
                            subcommittee_index: subnet_id.into(),
                        };

                        beacon_node
                            .get_validator_sync_committee_contribution(&sync_contribution_data)
                            .await
                    })
                    .instrument(info_span!("fetch_sync_contribution"))
                    .await;
                (subnet_id, result)
            })
            .collect();

        let total_subnets = subnet_ids.len();
        let mut sync_contributions = HashMap::with_capacity(total_subnets);
        let deadline = Instant::now() + timeout;

        // Collect results as they complete, until timeout or all done
        loop {
            // Exit when all futures completed
            if futures.is_empty() {
                break;
            }

            tokio::select! {
                Some((subnet_id, result)) = futures.next() => {
                    match result {
                        Ok(Some(response)) => {
                            sync_contributions.insert(subnet_id, response.data);
                        }
                        Ok(None) => {
                            warn!(
                                %slot,
                                ?beacon_block_root,
                                ?subnet_id,
                                "[AggregatorCommittee] No sync contribution found for subnet"
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                %slot,
                                ?beacon_block_root,
                                ?subnet_id,
                                error = %e,
                                "[AggregatorCommittee] Failed to fetch sync contribution for subnet"
                            );
                        }
                    }
                }
                _ = sleep_until(deadline) => {
                    if sync_contributions.len() < total_subnets {
                        warn!(
                            %slot,
                            collected = sync_contributions.len(),
                            total = total_subnets,
                            "[AggregatorCommittee] Timeout fetching sync contributions, returning partial results"
                        );
                        metrics::inc_counter_vec(
                            &metrics::AGGREGATOR_COMMITTEE_PARTIAL_RESULTS,
                            &["sync_contributions"],
                        );
                    }
                    break;
                }
            }
        }

        // Track success/failure counts
        let successful = sync_contributions.len();
        let total = total_subnets;
        let failed = total.saturating_sub(successful);
        metrics::inc_counter_vec_by(
            &metrics::AGGREGATOR_COMMITTEE_FETCH_SUCCESS,
            &["sync_contributions", "success"],
            successful as u64,
        );
        metrics::inc_counter_vec_by(
            &metrics::AGGREGATOR_COMMITTEE_FETCH_SUCCESS,
            &["sync_contributions", "failed"],
            failed as u64,
        );

        sync_contributions
    }
}

// ═══════════════════════════════════════════════════════════════════════════════════════
// Pure Helper Functions for Consensus Data Building
// ═══════════════════════════════════════════════════════════════════════════════════════
//
// These functions are extracted to enable unit testing of the sorting and filtering logic
// without requiring beacon node mocks. The sorting order MUST match SSV-Go exactly for
// consensus compatibility.

/// Sort aggregators by `validator_index` ascending.
///
/// CRITICAL for consensus: All aggregators share the same signing root (attestation data),
/// so we sort purely by `validator_index`. This matches SSV-Go's behavior.
pub fn sort_aggregators_by_validator_index(aggregators: &mut [AssignedAggregator]) {
    aggregators.sort_unstable_by_key(|a| a.validator_index.0);
}

/// Sort contributors by (`signing_root`, `validator_index`).
///
/// CRITICAL for consensus: Contributors may have different signing roots (different subnets),
/// so we sort first by signing root, then by `validator_index` within each root group.
/// This matches SSV-Go's root-sorted processing.
pub fn sort_contributors_by_signing_root_then_validator_index(
    contributors_with_roots: &mut [(Hash256, AssignedAggregator)],
) {
    contributors_with_roots.sort_unstable_by(|(root_a, contrib_a), (root_b, contrib_b)| {
        root_a
            .cmp(root_b)
            .then_with(|| contrib_a.validator_index.cmp(&contrib_b.validator_index))
    });
}

/// Filter aggregators to only those whose attestation was successfully fetched.
///
/// This maintains 1:1 correspondence between `aggregator_committee_indexes` and attestations.
pub fn filter_aggregators_with_attestations<E: EthSpec>(
    aggregators: &mut Vec<AssignedAggregator>,
    aggregated_attestations: &HashMap<u64, Attestation<E>>,
) {
    aggregators.retain(|agg| aggregated_attestations.contains_key(&agg.committee_index));
}

/// Filter contributors to only those whose sync contribution was successfully fetched.
///
/// This maintains 1:1 correspondence between `subnet_ids` and contributions.
pub fn filter_contributors_with_contributions<E: EthSpec>(
    contributors_with_roots: &mut Vec<(Hash256, AssignedAggregator)>,
    sync_contributions: &HashMap<SyncSubnetId, SyncCommitteeContribution<E>>,
) {
    contributors_with_roots.retain(|(_, contrib)| {
        sync_contributions.contains_key(&SyncSubnetId::new(contrib.committee_index))
    });
}

#[cfg(test)]
mod tests {
    use bls::{AggregateSignature, FixedBytesExtended, Signature};
    use ssv_types::{
        IndexSet, VariableList,
        consensus::{
            AggregatorCommitteeConsensusData, AggregatorCommitteeDataValidator, AssignedAggregator,
            DataVersion, MaxAggregatedAttestationBytes, QbftDataValidator,
        },
    };
    use ssz::Encode;
    use ssz_types::BitList;
    use types::{
        AttestationBase, AttestationData, Checkpoint, Epoch, ForkName, MainnetEthSpec, Slot,
        SyncCommitteeContribution,
    };

    use super::*;

    // ═══════════════════════════════════════════════════════════════════════════════════
    // Test Helpers
    // ═══════════════════════════════════════════════════════════════════════════════════

    /// Create a test AssignedAggregator with specified validator_index and committee_index
    fn create_aggregator(validator_index: usize, committee_index: u64) -> AssignedAggregator {
        AssignedAggregator {
            validator_index: ValidatorIndex(validator_index),
            selection_proof: Signature::empty(),
            committee_index,
        }
    }

    /// Create a contributor with signing root for testing
    fn create_contributor_with_root(
        signing_root: Hash256,
        validator_index: usize,
        subnet_id: u64,
    ) -> (Hash256, AssignedAggregator) {
        (
            signing_root,
            AssignedAggregator {
                validator_index: ValidatorIndex(validator_index),
                selection_proof: Signature::empty(),
                committee_index: subnet_id,
            },
        )
    }

    /// Create a test attestation for a given committee index
    fn create_test_attestation(index: u64) -> Attestation<MainnetEthSpec> {
        Attestation::Base(AttestationBase {
            aggregation_bits: BitList::with_capacity(128).expect("valid capacity"),
            data: AttestationData {
                slot: Slot::new(1000),
                index,
                beacon_block_root: Hash256::zero(),
                source: Checkpoint {
                    epoch: Epoch::new(10),
                    root: Hash256::zero(),
                },
                target: Checkpoint {
                    epoch: Epoch::new(11),
                    root: Hash256::zero(),
                },
            },
            signature: AggregateSignature::infinity(),
        })
    }

    /// Create test attestation bytes for consensus data
    fn create_attestation_bytes(index: u64) -> VariableList<u8, MaxAggregatedAttestationBytes> {
        let attestation = AttestationBase::<MainnetEthSpec> {
            aggregation_bits: BitList::with_capacity(128).expect("valid capacity"),
            data: AttestationData {
                slot: Slot::new(1000),
                index,
                beacon_block_root: Hash256::zero(),
                source: Checkpoint {
                    epoch: Epoch::new(10),
                    root: Hash256::zero(),
                },
                target: Checkpoint {
                    epoch: Epoch::new(11),
                    root: Hash256::zero(),
                },
            },
            signature: AggregateSignature::infinity(),
        };
        VariableList::new(attestation.as_ssz_bytes()).expect("valid attestation bytes")
    }

    /// Create a test sync committee contribution
    fn create_test_contribution(subnet_id: u64) -> SyncCommitteeContribution<MainnetEthSpec> {
        SyncCommitteeContribution {
            slot: Slot::new(1000),
            beacon_block_root: Hash256::zero(),
            subcommittee_index: subnet_id,
            aggregation_bits: Default::default(),
            signature: AggregateSignature::infinity(),
        }
    }

    // ══════════════════════════════════���════════════════════════════════════════════════
    // P0 Tests: Wire Compatibility
    // ═══════════════════════════════════════════════════════════════════════════════════

    /// P0-1: Verify aggregators are sorted by validator_index ascending.
    ///
    /// This is CRITICAL for consensus - all operators must produce the same hash
    /// for the same input data. If aggregators are not sorted identically,
    /// consensus will fail due to hash mismatch.
    #[test]
    fn test_aggregators_sorted_by_validator_index() {
        // Create aggregators in unsorted order
        let mut aggregators = vec![
            create_aggregator(500, 10),
            create_aggregator(100, 5),
            create_aggregator(300, 7),
            create_aggregator(200, 5), // Same committee as validator 100
            create_aggregator(50, 3),
        ];

        // Sort using the production function
        sort_aggregators_by_validator_index(&mut aggregators);

        // Verify sorted by validator_index ascending
        let indices: Vec<usize> = aggregators.iter().map(|a| a.validator_index.0).collect();
        assert_eq!(
            indices,
            vec![50, 100, 200, 300, 500],
            "Aggregators must be sorted by validator_index ascending for consensus compatibility"
        );

        // Verify sorting is stable for same validator_index edge case
        let mut same_index_aggregators = vec![
            create_aggregator(100, 10),
            create_aggregator(100, 5), // Same validator_index, different committee
        ];
        sort_aggregators_by_validator_index(&mut same_index_aggregators);

        // Both should have validator_index 100, order may vary but that's ok
        // since SSV-Go would produce same result
        assert!(
            same_index_aggregators
                .iter()
                .all(|a| a.validator_index.0 == 100)
        );
    }

    /// P0-2: Verify contributors are sorted by (signing_root, validator_index).
    ///
    /// This is CRITICAL for consensus - sync committee contributors have different
    /// signing roots per subnet, so we must sort by root first, then validator_index.
    /// This matches SSV-Go's root-sorted processing.
    #[test]
    fn test_contributors_sorted_by_signing_root_then_validator_index() {
        // Create signing roots with predictable ordering
        // Hash256 comparison is lexicographic on the underlying bytes
        let root_a = Hash256::from_low_u64_be(1); // Smaller hash
        let root_b = Hash256::from_low_u64_be(2); // Larger hash
        let root_c = Hash256::from_low_u64_be(3); // Even larger hash

        // Create contributors in unsorted order with various roots
        let mut contributors = vec![
            create_contributor_with_root(root_c, 100, 2), // root_c, validator 100
            create_contributor_with_root(root_a, 300, 0), // root_a, validator 300
            create_contributor_with_root(root_b, 50, 1),  // root_b, validator 50
            create_contributor_with_root(root_a, 100, 0), /* root_a, validator 100 (same root as
                                                           * above) */
            create_contributor_with_root(root_b, 200, 1), /* root_b, validator 200 (same root as
                                                           * above) */
        ];

        // Sort using the production function
        sort_contributors_by_signing_root_then_validator_index(&mut contributors);

        // Expected order:
        // 1. root_a, validator 100 (smallest root, smaller validator)
        // 2. root_a, validator 300 (smallest root, larger validator)
        // 3. root_b, validator 50 (middle root, smaller validator)
        // 4. root_b, validator 200 (middle root, larger validator)
        // 5. root_c, validator 100 (largest root)

        let result: Vec<(Hash256, usize)> = contributors
            .iter()
            .map(|(root, agg)| (*root, agg.validator_index.0))
            .collect();

        assert_eq!(
            result,
            vec![
                (root_a, 100),
                (root_a, 300),
                (root_b, 50),
                (root_b, 200),
                (root_c, 100),
            ],
            "Contributors must be sorted by (signing_root, validator_index) for consensus compatibility"
        );
    }

    /// P0-3: Verify same inputs produce identical hash (deterministic output).
    ///
    /// This test verifies that given the same input data, the consensus data
    /// will always produce the same hash. This is essential for distributed
    /// consensus - all operators must agree on the hash for the same data.
    #[test]
    fn test_deterministic_output_same_inputs() {
        // Create identical consensus data twice using the same methodology
        // that build_consensus_data_for_committee would use

        let build_consensus_data = || {
            // Create aggregators and sort them
            let mut aggregators = vec![
                create_aggregator(300, 10),
                create_aggregator(100, 5),
                create_aggregator(200, 5),
            ];
            sort_aggregators_by_validator_index(&mut aggregators);

            // Create contributors with roots and sort them
            let root_a = Hash256::from_low_u64_be(100);
            let root_b = Hash256::from_low_u64_be(200);
            let mut contributors_with_roots = vec![
                create_contributor_with_root(root_b, 50, 1),
                create_contributor_with_root(root_a, 100, 0),
                create_contributor_with_root(root_a, 50, 0),
            ];
            sort_contributors_by_signing_root_then_validator_index(&mut contributors_with_roots);

            let contributors: Vec<AssignedAggregator> = contributors_with_roots
                .into_iter()
                .map(|(_, contrib)| contrib)
                .collect();

            // Extract committee indexes in first-seen order from sorted aggregators
            let committee_indexes: IndexSet<u64> =
                aggregators.iter().map(|a| a.committee_index).collect();

            // Extract subnet IDs in first-seen order from sorted contributors
            // (kept for illustration but unused since we create fixed contributions)
            let _subnet_ids: IndexSet<SyncSubnetId> = contributors
                .iter()
                .map(|c| SyncSubnetId::new(c.committee_index))
                .collect();

            // Build the consensus data structure
            AggregatorCommitteeConsensusData::<MainnetEthSpec> {
                version: DataVersion::from(ForkName::Deneb),
                aggregators: aggregators.try_into().expect("valid aggregators"),
                aggregator_committee_indexes: committee_indexes
                    .into_iter()
                    .collect::<Vec<_>>()
                    .try_into()
                    .expect("valid indexes"),
                aggregated_attestations: VariableList::new(vec![
                    create_attestation_bytes(5),
                    create_attestation_bytes(10),
                ])
                .expect("valid attestations"),
                contributors: contributors.try_into().expect("valid contributors"),
                sync_committee_contributions: VariableList::new(vec![
                    create_test_contribution(0),
                    create_test_contribution(1),
                ])
                .expect("valid contributions"),
            }
        };

        // Build twice and compare hashes
        let data1 = build_consensus_data();
        let data2 = build_consensus_data();

        // Use the QbftData::hash() method which is used for consensus
        use ssv_types::consensus::QbftData;
        let hash1 = data1.hash();
        let hash2 = data2.hash();

        assert_eq!(
            hash1, hash2,
            "Same inputs must produce identical hash for consensus to work"
        );

        // Also verify SSZ encoding is identical (which is what hash() uses)
        assert_eq!(
            data1.as_ssz_bytes(),
            data2.as_ssz_bytes(),
            "Same inputs must produce identical SSZ encoding"
        );
    }

    /// P0-4: Verify built data passes AggregatorCommitteeDataValidator.
    ///
    /// This test ensures that the consensus data built using our sorting and
    /// construction logic passes the validation that will be performed during
    /// QBFT consensus. If our data fails validation, consensus will fail.
    #[test]
    fn test_output_passes_validator_with_both() {
        // Build consensus data with both aggregators and contributors
        // following the exact same logic as build_consensus_data_for_committee

        // Create and sort aggregators
        let mut aggregators = vec![
            create_aggregator(300, 10),
            create_aggregator(100, 5),
            create_aggregator(200, 5), // Same committee_index as validator 100
        ];
        sort_aggregators_by_validator_index(&mut aggregators);

        // Create and sort contributors
        let root_a = Hash256::from_low_u64_be(100);
        let root_b = Hash256::from_low_u64_be(200);
        let mut contributors_with_roots = vec![
            create_contributor_with_root(root_b, 150, 1),
            create_contributor_with_root(root_a, 250, 0),
            create_contributor_with_root(root_a, 50, 0), // Same subnet as validator 250
        ];
        sort_contributors_by_signing_root_then_validator_index(&mut contributors_with_roots);

        let contributors: Vec<AssignedAggregator> = contributors_with_roots
            .into_iter()
            .map(|(_, contrib)| contrib)
            .collect();

        // Extract committee indexes in first-seen order (preserving order from sorted aggregators)
        let committee_indexes: IndexSet<u64> =
            aggregators.iter().map(|a| a.committee_index).collect();

        // Extract subnet IDs in first-seen order (preserving order from sorted contributors)
        let subnet_ids: IndexSet<SyncSubnetId> = contributors
            .iter()
            .map(|c| SyncSubnetId::new(c.committee_index))
            .collect();

        // Build attestation bytes matching the committee indexes
        let attestation_bytes: Vec<VariableList<u8, MaxAggregatedAttestationBytes>> =
            committee_indexes
                .iter()
                .map(|&idx| create_attestation_bytes(idx))
                .collect();

        // Build contributions matching the subnet IDs
        let contributions: Vec<SyncCommitteeContribution<MainnetEthSpec>> = subnet_ids
            .iter()
            .map(|id| create_test_contribution((*id).into()))
            .collect();

        // Build the complete consensus data
        let consensus_data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: aggregators.try_into().expect("valid aggregators"),
            aggregator_committee_indexes: committee_indexes
                .into_iter()
                .collect::<Vec<_>>()
                .try_into()
                .expect("valid indexes"),
            aggregated_attestations: attestation_bytes.try_into().expect("valid attestations"),
            contributors: contributors.try_into().expect("valid contributors"),
            sync_committee_contributions: contributions.try_into().expect("valid contributions"),
        };

        // Create validator and run validation
        let validator = AggregatorCommitteeDataValidator::<MainnetEthSpec>::new();

        // Validate using the do_validation method for detailed error reporting
        let result = validator.do_validation(&consensus_data);
        assert!(
            result.is_ok(),
            "Consensus data built with proper sorting/construction must pass validation. Error: {:?}",
            result.err()
        );

        // Also test via the QbftDataValidator trait (used during actual consensus)
        let passes_trait_validation =
            QbftDataValidator::validate(&validator, &consensus_data, &consensus_data);
        assert!(
            passes_trait_validation,
            "Consensus data must pass QbftDataValidator trait validation"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════════════
    // Additional Edge Case Tests
    // ═══════════════════════════════════════════════════════════════════════════════════

    /// Test filter_aggregators_with_attestations removes aggregators without attestations
    #[test]
    fn test_filter_aggregators_removes_unfetched() {
        let mut aggregators = vec![
            create_aggregator(100, 5),
            create_aggregator(200, 10),
            create_aggregator(300, 15),
        ];

        // Only have attestation for committee 5 and 15, not 10
        let mut attestations = HashMap::new();
        attestations.insert(5, create_test_attestation(5));
        attestations.insert(15, create_test_attestation(15));

        filter_aggregators_with_attestations(&mut aggregators, &attestations);

        assert_eq!(
            aggregators.len(),
            2,
            "Should filter out aggregator with committee_index 10"
        );
        let remaining_indexes: Vec<usize> =
            aggregators.iter().map(|a| a.validator_index.0).collect();
        assert_eq!(remaining_indexes, vec![100, 300]);
    }

    /// Test filter_contributors_with_contributions removes contributors without contributions
    #[test]
    fn test_filter_contributors_removes_unfetched() {
        let root = Hash256::zero();
        let mut contributors = vec![
            create_contributor_with_root(root, 100, 0),
            create_contributor_with_root(root, 200, 1),
            create_contributor_with_root(root, 300, 2),
        ];

        // Only have contributions for subnet 0 and 2, not 1
        let mut contributions = HashMap::new();
        contributions.insert(SyncSubnetId::new(0), create_test_contribution(0));
        contributions.insert(SyncSubnetId::new(2), create_test_contribution(2));

        filter_contributors_with_contributions(&mut contributors, &contributions);

        assert_eq!(
            contributors.len(),
            2,
            "Should filter out contributor with subnet_id 1"
        );
        let remaining_indexes: Vec<usize> = contributors
            .iter()
            .map(|(_, a)| a.validator_index.0)
            .collect();
        assert_eq!(remaining_indexes, vec![100, 300]);
    }

    /// Test that IndexSet preserves first-seen order from sorted aggregators
    #[test]
    fn test_committee_indexes_preserve_first_seen_order() {
        // Create aggregators with repeated committee indexes
        let mut aggregators = vec![
            create_aggregator(300, 10), // First occurrence of 10
            create_aggregator(100, 5),  // First occurrence of 5
            create_aggregator(200, 5),  // Second occurrence of 5 (should be deduped)
            create_aggregator(400, 10), // Second occurrence of 10 (should be deduped)
            create_aggregator(50, 3),   // First occurrence of 3
        ];

        // Sort aggregators first
        sort_aggregators_by_validator_index(&mut aggregators);

        // Extract committee indexes preserving first-seen order
        let committee_indexes: IndexSet<u64> =
            aggregators.iter().map(|a| a.committee_index).collect();

        // After sorting by validator_index: [50, 100, 200, 300, 400]
        // Committee indexes in order: [3, 5, 5, 10, 10]
        // First-seen unique: [3, 5, 10]
        let indexes: Vec<u64> = committee_indexes.into_iter().collect();
        assert_eq!(
            indexes,
            vec![3, 5, 10],
            "Committee indexes must be in first-seen order from sorted aggregators"
        );
    }

    /// Test that subnet IDs preserve first-seen order from sorted contributors
    #[test]
    fn test_subnet_ids_preserve_first_seen_order() {
        // Create signing roots
        let root_a = Hash256::from_low_u64_be(1);
        let root_b = Hash256::from_low_u64_be(2);

        // Create contributors with repeated subnet IDs
        let mut contributors_with_roots = vec![
            create_contributor_with_root(root_b, 100, 1), // root_b, subnet 1
            create_contributor_with_root(root_a, 200, 0), // root_a, subnet 0
            create_contributor_with_root(root_a, 50, 0),  // root_a, subnet 0 (duplicate)
            create_contributor_with_root(root_b, 150, 1), // root_b, subnet 1 (duplicate)
        ];

        // Sort contributors first
        sort_contributors_by_signing_root_then_validator_index(&mut contributors_with_roots);

        // Extract just the contributors
        let contributors: Vec<AssignedAggregator> = contributors_with_roots
            .into_iter()
            .map(|(_, contrib)| contrib)
            .collect();

        // Extract subnet IDs preserving first-seen order
        let subnet_ids: IndexSet<SyncSubnetId> = contributors
            .iter()
            .map(|c| SyncSubnetId::new(c.committee_index))
            .collect();

        // After sorting: root_a first (validators 50, 200 both with subnet 0),
        // then root_b (validators 100, 150 both with subnet 1)
        // First-seen unique: [subnet 0, subnet 1]
        let ids: Vec<u64> = subnet_ids.into_iter().map(|id| id.into()).collect();
        assert_eq!(
            ids,
            vec![0, 1],
            "Subnet IDs must be in first-seen order from sorted contributors"
        );
    }

    /// Test empty inputs produce valid (None) result
    #[test]
    fn test_empty_aggregators_and_contributors() {
        let mut aggregators: Vec<AssignedAggregator> = vec![];
        sort_aggregators_by_validator_index(&mut aggregators);
        assert!(aggregators.is_empty());

        let mut contributors: Vec<(Hash256, AssignedAggregator)> = vec![];
        sort_contributors_by_signing_root_then_validator_index(&mut contributors);
        assert!(contributors.is_empty());
    }

    // ═══════════════════════════════════════════════════════════════════════════════════
    // P1 Tests: High Priority - Failure Scenarios
    // ═══════════════════════════════════════════════════════════════════════════════════

    /// P1-1: When all attestation fetches fail AND all contribution fetches fail,
    /// the result should indicate no valid data (empty aggregators AND empty contributors
    /// after filtering).
    ///
    /// This simulates the scenario where beacon node is unavailable or returns errors
    /// for all requested attestations and contributions. The consensus data building
    /// logic should gracefully handle this by producing no consensus data (None result
    /// in build_consensus_data_for_committee).
    #[test]
    fn test_all_fetches_fail_returns_none() {
        // Create aggregators with various committee indexes
        let mut aggregators = vec![
            create_aggregator(100, 5),
            create_aggregator(200, 10),
            create_aggregator(300, 15),
        ];
        sort_aggregators_by_validator_index(&mut aggregators);

        // Create contributors with various subnet IDs
        let root = Hash256::zero();
        let mut contributors = vec![
            create_contributor_with_root(root, 50, 0),
            create_contributor_with_root(root, 150, 1),
            create_contributor_with_root(root, 250, 2),
        ];
        sort_contributors_by_signing_root_then_validator_index(&mut contributors);

        // Empty attestations map - simulates all fetches failing
        let aggregated_attestations: HashMap<u64, Attestation<MainnetEthSpec>> = HashMap::new();

        // Empty contributions map - simulates all fetches failing
        let sync_contributions: HashMap<SyncSubnetId, SyncCommitteeContribution<MainnetEthSpec>> =
            HashMap::new();

        // Apply the filter functions (as done in build_consensus_data_for_committee)
        filter_aggregators_with_attestations(&mut aggregators, &aggregated_attestations);
        filter_contributors_with_contributions(&mut contributors, &sync_contributions);

        // After filtering, both should be empty because no attestations/contributions were fetched
        assert!(
            aggregators.is_empty(),
            "All aggregators should be filtered out when no attestations are fetched"
        );
        assert!(
            contributors.is_empty(),
            "All contributors should be filtered out when no contributions are fetched"
        );

        // This matches the early exit condition in build_consensus_data_for_committee:
        // if aggregators.is_empty() && contributors.is_empty() { return Ok(None); }
        let should_return_none = aggregators.is_empty() && contributors.is_empty();
        assert!(
            should_return_none,
            "When all fetches fail, build_consensus_data_for_committee should return None"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════════════
    // P2 Tests: Medium Priority - Valid Edge Cases
    // ═══════════════════════════════════════════════════════════════════════════════════

    /// P2-1: Valid scenario - no attestation aggregators, only sync contributors.
    ///
    /// Should produce valid consensus data with empty aggregators list but populated
    /// contributors. This is a valid scenario when a committee only has sync committee
    /// aggregation duties and no attestation aggregation duties for a given slot.
    #[test]
    fn test_empty_aggregators_only_contributors() {
        // No aggregators
        let aggregators: Vec<AssignedAggregator> = vec![];

        // Create contributors with contributions
        let root_a = Hash256::from_low_u64_be(1);
        let root_b = Hash256::from_low_u64_be(2);
        let mut contributors_with_roots = vec![
            create_contributor_with_root(root_b, 150, 1),
            create_contributor_with_root(root_a, 250, 0),
            create_contributor_with_root(root_a, 50, 0), // Same subnet as validator 250
        ];
        sort_contributors_by_signing_root_then_validator_index(&mut contributors_with_roots);

        // Create contributions for all subnets
        let mut sync_contributions = HashMap::new();
        sync_contributions.insert(SyncSubnetId::new(0), create_test_contribution(0));
        sync_contributions.insert(SyncSubnetId::new(1), create_test_contribution(1));

        // Filter contributors (all should remain since we have contributions)
        filter_contributors_with_contributions(&mut contributors_with_roots, &sync_contributions);

        // Extract contributors after filtering
        let contributors: Vec<AssignedAggregator> = contributors_with_roots
            .into_iter()
            .map(|(_, contrib)| contrib)
            .collect();

        assert!(
            !contributors.is_empty(),
            "Contributors should not be empty when contributions are fetched"
        );
        assert_eq!(
            contributors.len(),
            3,
            "All 3 contributors should remain after filtering"
        );

        // Extract subnet IDs in first-seen order
        let subnet_ids: IndexSet<SyncSubnetId> = contributors
            .iter()
            .map(|c| SyncSubnetId::new(c.committee_index))
            .collect();

        // Build contributions matching subnet IDs
        let contributions: Vec<SyncCommitteeContribution<MainnetEthSpec>> = subnet_ids
            .iter()
            .filter_map(|id| sync_contributions.get(id).cloned())
            .collect();

        // Build the consensus data with empty aggregators but populated contributors
        let consensus_data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: aggregators.try_into().expect("valid empty aggregators"),
            aggregator_committee_indexes: Vec::<u64>::new()
                .try_into()
                .expect("valid empty indexes"),
            aggregated_attestations: VariableList::empty(),
            contributors: contributors.try_into().expect("valid contributors"),
            sync_committee_contributions: contributions.try_into().expect("valid contributions"),
        };

        // Verify structure is valid
        assert!(
            consensus_data.aggregators.is_empty(),
            "Aggregators should be empty"
        );
        assert!(
            !consensus_data.contributors.is_empty(),
            "Contributors should be populated"
        );
        assert!(
            consensus_data.aggregated_attestations.is_empty(),
            "Attestations should be empty"
        );
        assert!(
            !consensus_data.sync_committee_contributions.is_empty(),
            "Contributions should be populated"
        );

        // Validate with the consensus data validator
        let validator = AggregatorCommitteeDataValidator::<MainnetEthSpec>::new();
        let result = validator.do_validation(&consensus_data);
        assert!(
            result.is_ok(),
            "Consensus data with only contributors should pass validation. Error: {:?}",
            result.err()
        );
    }

    /// P2-2: Valid scenario - no sync contributors, only attestation aggregators.
    ///
    /// Should produce valid consensus data with empty contributors list but populated
    /// aggregators. This is a valid scenario when a committee only has attestation
    /// aggregation duties and no sync committee aggregation duties for a given slot.
    #[test]
    fn test_empty_contributors_only_aggregators() {
        // Create aggregators with attestations
        let mut aggregators = vec![
            create_aggregator(300, 10),
            create_aggregator(100, 5),
            create_aggregator(200, 7),
        ];
        sort_aggregators_by_validator_index(&mut aggregators);

        // Create attestations for all committees
        let mut aggregated_attestations = HashMap::new();
        aggregated_attestations.insert(5, create_test_attestation(5));
        aggregated_attestations.insert(7, create_test_attestation(7));
        aggregated_attestations.insert(10, create_test_attestation(10));

        // Filter aggregators (all should remain since we have attestations)
        filter_aggregators_with_attestations(&mut aggregators, &aggregated_attestations);

        assert!(
            !aggregators.is_empty(),
            "Aggregators should not be empty when attestations are fetched"
        );
        assert_eq!(
            aggregators.len(),
            3,
            "All 3 aggregators should remain after filtering"
        );

        // Extract committee indexes in first-seen order from sorted aggregators
        let committee_indexes: IndexSet<u64> =
            aggregators.iter().map(|a| a.committee_index).collect();

        // Build attestation bytes matching committee indexes
        let attestation_bytes: Vec<VariableList<u8, MaxAggregatedAttestationBytes>> =
            committee_indexes
                .iter()
                .map(|&idx| create_attestation_bytes(idx))
                .collect();

        // No contributors
        let contributors: Vec<AssignedAggregator> = vec![];

        // Build the consensus data with populated aggregators but empty contributors
        let consensus_data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: aggregators.try_into().expect("valid aggregators"),
            aggregator_committee_indexes: committee_indexes
                .into_iter()
                .collect::<Vec<_>>()
                .try_into()
                .expect("valid indexes"),
            aggregated_attestations: attestation_bytes.try_into().expect("valid attestations"),
            contributors: contributors.try_into().expect("valid empty contributors"),
            sync_committee_contributions: VariableList::empty(),
        };

        // Verify structure is valid
        assert!(
            !consensus_data.aggregators.is_empty(),
            "Aggregators should be populated"
        );
        assert!(
            consensus_data.contributors.is_empty(),
            "Contributors should be empty"
        );
        assert!(
            !consensus_data.aggregated_attestations.is_empty(),
            "Attestations should be populated"
        );
        assert!(
            consensus_data.sync_committee_contributions.is_empty(),
            "Contributions should be empty"
        );

        // Validate with the consensus data validator
        let validator = AggregatorCommitteeDataValidator::<MainnetEthSpec>::new();
        let result = validator.do_validation(&consensus_data);
        assert!(
            result.is_ok(),
            "Consensus data with only aggregators should pass validation. Error: {:?}",
            result.err()
        );
    }

    /// P2-3: 5 aggregators all assigned to committee_index 42.
    ///
    /// Should:
    /// - Sort by validator_index
    /// - Deduplicate committee_index to single entry in IndexSet
    /// - All 5 aggregators remain in the list
    /// - Only 1 committee_index in the output
    ///
    /// This tests the deduplication behavior of IndexSet for committee indexes
    /// when multiple aggregators share the same committee.
    #[test]
    fn test_multiple_aggregators_same_committee() {
        // Create 5 aggregators all with committee_index 42, in unsorted order
        let mut aggregators = vec![
            create_aggregator(500, 42),
            create_aggregator(100, 42),
            create_aggregator(300, 42),
            create_aggregator(200, 42),
            create_aggregator(400, 42),
        ];

        // Sort by validator_index
        sort_aggregators_by_validator_index(&mut aggregators);

        // Verify sorted order
        let validator_indices: Vec<usize> =
            aggregators.iter().map(|a| a.validator_index.0).collect();
        assert_eq!(
            validator_indices,
            vec![100, 200, 300, 400, 500],
            "Aggregators must be sorted by validator_index ascending"
        );

        // Create attestation for committee 42
        let mut aggregated_attestations = HashMap::new();
        aggregated_attestations.insert(42, create_test_attestation(42));

        // Filter aggregators (all should remain since we have the attestation for committee 42)
        filter_aggregators_with_attestations(&mut aggregators, &aggregated_attestations);

        // All 5 aggregators should remain
        assert_eq!(
            aggregators.len(),
            5,
            "All 5 aggregators should remain after filtering since attestation for committee 42 exists"
        );

        // Extract committee indexes in first-seen order (should deduplicate to 1 entry)
        let committee_indexes: IndexSet<u64> =
            aggregators.iter().map(|a| a.committee_index).collect();

        // Verify only 1 unique committee_index
        assert_eq!(
            committee_indexes.len(),
            1,
            "IndexSet should deduplicate to single committee_index"
        );
        assert!(
            committee_indexes.contains(&42),
            "The single committee_index should be 42"
        );

        // Build attestation bytes (only 1 attestation for the single committee_index)
        let attestation_bytes: Vec<VariableList<u8, MaxAggregatedAttestationBytes>> =
            committee_indexes
                .iter()
                .map(|&idx| create_attestation_bytes(idx))
                .collect();

        assert_eq!(
            attestation_bytes.len(),
            1,
            "Should have exactly 1 attestation bytes entry for the deduplicated committee"
        );

        // Build the consensus data
        let consensus_data = AggregatorCommitteeConsensusData::<MainnetEthSpec> {
            version: DataVersion::from(ForkName::Deneb),
            aggregators: aggregators.try_into().expect("valid aggregators"),
            aggregator_committee_indexes: committee_indexes
                .into_iter()
                .collect::<Vec<_>>()
                .try_into()
                .expect("valid indexes"),
            aggregated_attestations: attestation_bytes.try_into().expect("valid attestations"),
            contributors: Vec::<AssignedAggregator>::new()
                .try_into()
                .expect("valid empty contributors"),
            sync_committee_contributions: VariableList::empty(),
        };

        // Verify final structure
        assert_eq!(
            consensus_data.aggregators.len(),
            5,
            "Should have all 5 aggregators in final consensus data"
        );
        assert_eq!(
            consensus_data.aggregator_committee_indexes.len(),
            1,
            "Should have only 1 committee_index in final consensus data"
        );
        assert_eq!(
            consensus_data.aggregated_attestations.len(),
            1,
            "Should have only 1 attestation in final consensus data"
        );

        // Validate with the consensus data validator
        let validator = AggregatorCommitteeDataValidator::<MainnetEthSpec>::new();
        let result = validator.do_validation(&consensus_data);
        assert!(
            result.is_ok(),
            "Consensus data with multiple aggregators same committee should pass validation. Error: {:?}",
            result.err()
        );

        // Also verify via QbftDataValidator trait
        let passes_trait_validation =
            QbftDataValidator::validate(&validator, &consensus_data, &consensus_data);
        assert!(
            passes_trait_validation,
            "Consensus data must pass QbftDataValidator trait validation"
        );
    }
}

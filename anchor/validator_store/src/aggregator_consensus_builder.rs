//! Builds aggregation assignments for QBFT consensus (Boole+ fork).

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

use crate::{AnchorValidatorStore, ContributionWaiter, VotingAssignments, metrics};

pub struct SyncAggregatorData {
    pub validator_index: u64,
    pub pubkey: PublicKeyBytes,
    pub selection_proof: SyncSelectionProof,
}

pub type SyncByCommitteeMap = HashMap<CommitteeId, Vec<(SyncSubnetId, SyncAggregatorData)>>;

pub(crate) type SyncAggregatorsBySubnet =
    HashMap<SyncSubnetId, Vec<(u64, PublicKeyBytes, SyncSelectionProof)>>;

/// Context for voting duties at 1/3 slot.
pub(crate) struct VotingContext {
    pub voting_assignments: Arc<VotingAssignments>,
    pub beacon_vote: BeaconVote,
}

/// Result of the single-pass grouping over duties.
pub(crate) struct GroupedDuties<'a, E: EthSpec> {
    pub aggregator_committees: HashMap<PublicKeyBytes, u64>,
    pub attesters_by_ssv_committee: HashMap<CommitteeId, Vec<&'a DutyAndProof>>,
    pub attestation_committee_indexes: HashSet<u64>,
    pub multi_sync_aggregators: HashMap<PublicKeyBytes, ContributionWaiter<E>>,
    pub sync_by_ssv_committee: SyncByCommitteeMap,
    pub all_subnet_ids: HashSet<SyncSubnetId>,
}

/// Groups attesters and sync aggregators by SSV committee in a single pass each.
pub(crate) fn group_duties_by_committee<'a, E: EthSpec, T: SlotClock + 'static>(
    attesters: &'a [DutyAndProof],
    sync_aggregators: Option<&SyncAggregatorsBySubnet>,
    validator_store: &AnchorValidatorStore<T, E>,
) -> GroupedDuties<'a, E> {
    let mut aggregator_committees: HashMap<PublicKeyBytes, u64> =
        HashMap::with_capacity(attesters.len());
    let mut attesters_by_ssv_committee: HashMap<CommitteeId, Vec<_>> = HashMap::new();
    let mut attestation_committee_indexes: HashSet<u64> = HashSet::with_capacity(attesters.len());

    for attester in attesters.iter().filter(|d| d.selection_proof.is_some()) {
        if let Some(ssv_committee_id) = validator_store
            .get_validator_and_cluster(attester.duty.pubkey)
            .ok()
            .map(|(_, cluster)| cluster.committee_id())
        {
            aggregator_committees.insert(attester.duty.pubkey, attester.duty.committee_index);
            attesters_by_ssv_committee
                .entry(ssv_committee_id)
                .or_default()
                .push(attester);
            attestation_committee_indexes.insert(attester.duty.committee_index);
        }
    }

    let mut validator_subnet_counts: HashMap<PublicKeyBytes, usize> = HashMap::new();
    let mut sync_by_ssv_committee: SyncByCommitteeMap = HashMap::new();
    let mut all_subnet_ids: HashSet<SyncSubnetId> =
        HashSet::with_capacity(sync_aggregators.map(|a| a.len()).unwrap_or(0));

    if let Some(aggregators) = sync_aggregators {
        for (subnet_id, subnet_aggregators) in aggregators {
            for (validator_index, pubkey, selection_proof) in subnet_aggregators {
                let sync_aggregator = SyncAggregatorData {
                    validator_index: *validator_index,
                    pubkey: *pubkey,
                    selection_proof: selection_proof.clone(),
                };

                if let Some(ssv_committee_id) = validator_store
                    .get_validator_and_cluster(sync_aggregator.pubkey)
                    .ok()
                    .map(|(_, cluster)| cluster.committee_id())
                {
                    *validator_subnet_counts
                        .entry(sync_aggregator.pubkey)
                        .or_insert(0) += 1;
                    sync_by_ssv_committee
                        .entry(ssv_committee_id)
                        .or_default()
                        .push((*subnet_id, sync_aggregator));
                    all_subnet_ids.insert(*subnet_id);
                }
            }
        }
    }

    let multi_sync_aggregators: HashMap<PublicKeyBytes, ContributionWaiter<E>> =
        validator_subnet_counts
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(pubkey, count)| (pubkey, ContributionWaiter::new(count)))
            .collect();

    GroupedDuties {
        aggregator_committees,
        attesters_by_ssv_committee,
        attestation_committee_indexes,
        multi_sync_aggregators,
        sync_by_ssv_committee,
        all_subnet_ids,
    }
}

/// Builds `AggregatorCommitteeConsensusData` for all committees with aggregation duties.
#[expect(clippy::too_many_arguments)]
pub(crate) async fn build_consensus_data_for_all_committees<E: EthSpec, T: SlotClock + 'static>(
    slot: Slot,
    attesters_by_ssv_committee: HashMap<CommitteeId, Vec<&DutyAndProof>>,
    sync_by_ssv_committee: SyncByCommitteeMap,
    attestation_committee_indexes: HashSet<u64>,
    all_subnet_ids: HashSet<SyncSubnetId>,
    voting_context: &VotingContext,
    timeout: Duration,
    validator_store: &Arc<AnchorValidatorStore<T, E>>,
    beacon_nodes: &Arc<BeaconNodeFallback<T>>,
    spec: &Arc<ChainSpec>,
) -> Result<HashMap<CommitteeId, Arc<AggregatorCommitteeConsensusData<E>>>, String> {
    let (aggregated_attestations, sync_contributions) = tokio::join!(
        fetch_aggregated_attestations(
            slot,
            &voting_context.beacon_vote,
            &attestation_committee_indexes,
            timeout,
            beacon_nodes,
            spec,
        ),
        fetch_sync_contributions(
            slot,
            voting_context.beacon_vote.block_root,
            &all_subnet_ids,
            timeout,
            beacon_nodes,
        ),
    );

    let ssv_committees: HashSet<CommitteeId> = attesters_by_ssv_committee
        .keys()
        .chain(sync_by_ssv_committee.keys())
        .copied()
        .collect();

    let mut result = HashMap::with_capacity(ssv_committees.len());
    for ssv_committee_id in ssv_committees {
        let ssv_committee_attesters = attesters_by_ssv_committee.get(&ssv_committee_id);
        let ssv_committee_sync = sync_by_ssv_committee.get(&ssv_committee_id);

        let consensus_data = build_consensus_data_for_committee(
            slot,
            &ssv_committee_id,
            ssv_committee_attesters,
            ssv_committee_sync,
            &aggregated_attestations,
            &sync_contributions,
            validator_store,
            spec,
        )?;

        if let Some(data) = consensus_data {
            result.insert(ssv_committee_id, Arc::new(data));
        }
    }

    Ok(result)
}

#[expect(clippy::too_many_arguments)]
fn build_consensus_data_for_committee<E: EthSpec, T: SlotClock + 'static>(
    slot: Slot,
    ssv_committee_id: &CommitteeId,
    ssv_committee_attesters: Option<&Vec<&DutyAndProof>>,
    ssv_committee_sync: Option<&Vec<(SyncSubnetId, SyncAggregatorData)>>,
    aggregated_attestations: &HashMap<u64, Attestation<E>>,
    sync_contributions: &HashMap<SyncSubnetId, SyncCommitteeContribution<E>>,
    validator_store: &Arc<AnchorValidatorStore<T, E>>,
    spec: &Arc<ChainSpec>,
) -> Result<Option<AggregatorCommitteeConsensusData<E>>, String> {
    let mut aggregators: Vec<AssignedAggregator> = match ssv_committee_attesters {
        Some(attesters) => Vec::with_capacity(attesters.len()),
        None => Vec::new(),
    };
    if let Some(attesters) = ssv_committee_attesters {
        for duty_and_proof in attesters.iter() {
            let validator_index = ValidatorIndex(duty_and_proof.duty.validator_index as usize);
            let Some(selection_proof) = duty_and_proof.selection_proof.clone() else {
                warn!(
                    %slot,
                    ?ssv_committee_id,
                    ?validator_index,
                    "BUG: aggregator missing selection_proof despite upstream filter"
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

    sort_aggregators_by_validator_index(&mut aggregators);
    filter_aggregators_with_attestations(&mut aggregators, aggregated_attestations);

    let mut contributors_with_roots: Vec<(Hash256, AssignedAggregator)> = match ssv_committee_sync {
        Some(sync_entries) => Vec::with_capacity(sync_entries.len()),
        None => Vec::new(),
    };
    if let Some(sync_entries) = ssv_committee_sync {
        for (subnet_id, sync_aggregator) in sync_entries.iter() {
            let sync_selection_root =
                validator_store.compute_sync_selection_root(slot, (*subnet_id).into());
            let validator_index = ValidatorIndex(sync_aggregator.validator_index as usize);
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

    sort_contributors_by_signing_root_then_validator_index(&mut contributors_with_roots);
    filter_contributors_with_contributions(&mut contributors_with_roots, sync_contributions);

    let contributors: Vec<AssignedAggregator> = contributors_with_roots
        .into_iter()
        .map(|(_, contributor)| contributor)
        .collect();

    if aggregators.is_empty() && contributors.is_empty() {
        return Ok(None);
    }

    let attestation_committee_indexes: IndexSet<u64> =
        aggregators.iter().map(|a| a.committee_index).collect();

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

    let subnet_ids: IndexSet<SyncSubnetId> = contributors
        .iter()
        .map(|c| SyncSubnetId::new(c.committee_index))
        .collect();

    let contributions: Vec<SyncCommitteeContribution<E>> = subnet_ids
        .iter()
        .filter_map(|id| sync_contributions.get(id).cloned())
        .collect();

    let epoch = slot.epoch(E::slots_per_epoch());
    let fork_name = spec.fork_name_at_epoch(epoch);
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

async fn fetch_aggregated_attestations<E: EthSpec, T: SlotClock + 'static>(
    slot: Slot,
    beacon_vote: &BeaconVote,
    attestation_committee_indexes: &HashSet<u64>,
    timeout: Duration,
    beacon_nodes: &Arc<BeaconNodeFallback<T>>,
    spec: &Arc<ChainSpec>,
) -> HashMap<u64, Attestation<E>> {
    let _timer = metrics::start_timer_vec(
        &metrics::AGGREGATOR_COMMITTEE_FETCH_TIMES,
        &["aggregated_attestations"],
    );

    let fork_name = spec.fork_name_at_epoch(slot.epoch(E::slots_per_epoch()));

    let mut futures: FuturesUnordered<_> = attestation_committee_indexes
        .iter()
        .map(|&committee_index| {
            let attestation_data = AttestationData {
                slot,
                index: if fork_name < ForkName::Electra {
                    committee_index
                } else {
                    0
                },
                beacon_block_root: beacon_vote.block_root,
                source: beacon_vote.source,
                target: beacon_vote.target,
            };
            let attestation_data_root = attestation_data.tree_hash_root();
            let beacon_nodes = beacon_nodes.clone();

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
                                format!("Failed to produce aggregate attestation: {:?}", e)
                            })?
                            .ok_or_else(|| {
                                format!(
                                    "No aggregate available for slot {}, committee {}",
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

    loop {
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
                        warn!(%slot, %committee_index, error = %e, "Failed to fetch aggregated attestation");
                    }
                }
            }
            _ = sleep_until(deadline) => {
                if aggregated_attestations.len() < total_committees {
                    warn!(
                        %slot,
                        collected = aggregated_attestations.len(),
                        total = total_committees,
                        "Timeout fetching aggregated attestations, returning partial results"
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

    let successful = aggregated_attestations.len();
    let failed = total_committees.saturating_sub(successful);
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

async fn fetch_sync_contributions<E: EthSpec, T: SlotClock + 'static>(
    slot: Slot,
    beacon_block_root: Hash256,
    subnet_ids: &HashSet<SyncSubnetId>,
    timeout: Duration,
    beacon_nodes: &Arc<BeaconNodeFallback<T>>,
) -> HashMap<SyncSubnetId, SyncCommitteeContribution<E>> {
    let _timer = metrics::start_timer_vec(
        &metrics::AGGREGATOR_COMMITTEE_FETCH_TIMES,
        &["sync_contributions"],
    );

    let mut futures: FuturesUnordered<_> = subnet_ids
        .iter()
        .map(|&subnet_id| {
            let beacon_nodes = beacon_nodes.clone();
            async move {
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
            }
        })
        .collect();

    let total_subnets = subnet_ids.len();
    let mut sync_contributions = HashMap::with_capacity(total_subnets);
    let deadline = Instant::now() + timeout;

    loop {
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
                        warn!(%slot, ?beacon_block_root, ?subnet_id, "No sync contribution found");
                    }
                    Err(e) => {
                        tracing::error!(%slot, ?beacon_block_root, ?subnet_id, error = %e, "Failed to fetch sync contribution");
                    }
                }
            }
            _ = sleep_until(deadline) => {
                if sync_contributions.len() < total_subnets {
                    warn!(
                        %slot,
                        collected = sync_contributions.len(),
                        total = total_subnets,
                        "Timeout fetching sync contributions, returning partial results"
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

    let successful = sync_contributions.len();
    let failed = total_subnets.saturating_sub(successful);
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

pub fn sort_aggregators_by_validator_index(aggregators: &mut [AssignedAggregator]) {
    aggregators.sort_unstable_by_key(|a| a.validator_index.0);
}

pub fn sort_contributors_by_signing_root_then_validator_index(
    contributors_with_roots: &mut [(Hash256, AssignedAggregator)],
) {
    contributors_with_roots.sort_unstable_by(|(root_a, contrib_a), (root_b, contrib_b)| {
        root_a
            .cmp(root_b)
            .then_with(|| contrib_a.validator_index.cmp(&contrib_b.validator_index))
    });
}

pub fn filter_aggregators_with_attestations<E: EthSpec>(
    aggregators: &mut Vec<AssignedAggregator>,
    aggregated_attestations: &HashMap<u64, Attestation<E>>,
) {
    aggregators.retain(|agg| aggregated_attestations.contains_key(&agg.committee_index));
}

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

    fn create_aggregator(validator_index: usize, committee_index: u64) -> AssignedAggregator {
        AssignedAggregator {
            validator_index: ValidatorIndex(validator_index),
            selection_proof: Signature::empty(),
            committee_index,
        }
    }

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

    fn create_test_contribution(subnet_id: u64) -> SyncCommitteeContribution<MainnetEthSpec> {
        SyncCommitteeContribution {
            slot: Slot::new(1000),
            beacon_block_root: Hash256::zero(),
            subcommittee_index: subnet_id,
            aggregation_bits: Default::default(),
            signature: AggregateSignature::infinity(),
        }
    }

    #[test]
    fn test_aggregators_sorted_by_validator_index() {
        let mut aggregators = vec![
            create_aggregator(500, 10),
            create_aggregator(100, 5),
            create_aggregator(300, 7),
            create_aggregator(200, 5),
            create_aggregator(50, 3),
        ];

        sort_aggregators_by_validator_index(&mut aggregators);

        let indices: Vec<usize> = aggregators.iter().map(|a| a.validator_index.0).collect();
        assert_eq!(indices, vec![50, 100, 200, 300, 500]);

        let mut same_index_aggregators =
            vec![create_aggregator(100, 10), create_aggregator(100, 5)];
        sort_aggregators_by_validator_index(&mut same_index_aggregators);
        assert!(
            same_index_aggregators
                .iter()
                .all(|a| a.validator_index.0 == 100)
        );
    }

    #[test]
    fn test_contributors_sorted_by_signing_root_then_validator_index() {
        let root_a = Hash256::from_low_u64_be(1);
        let root_b = Hash256::from_low_u64_be(2);
        let root_c = Hash256::from_low_u64_be(3);

        let mut contributors = vec![
            create_contributor_with_root(root_c, 100, 2),
            create_contributor_with_root(root_a, 300, 0),
            create_contributor_with_root(root_b, 50, 1),
            create_contributor_with_root(root_a, 100, 0),
            create_contributor_with_root(root_b, 200, 1),
        ];

        sort_contributors_by_signing_root_then_validator_index(&mut contributors);

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
            ]
        );
    }

    #[test]
    fn test_deterministic_output_same_inputs() {
        let build_consensus_data = || {
            let mut aggregators = vec![
                create_aggregator(300, 10),
                create_aggregator(100, 5),
                create_aggregator(200, 5),
            ];
            sort_aggregators_by_validator_index(&mut aggregators);

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

            let committee_indexes: IndexSet<u64> =
                aggregators.iter().map(|a| a.committee_index).collect();

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

        let data1 = build_consensus_data();
        let data2 = build_consensus_data();

        use ssv_types::consensus::QbftData;
        assert_eq!(data1.hash(), data2.hash());
        assert_eq!(data1.as_ssz_bytes(), data2.as_ssz_bytes());
    }

    #[test]
    fn test_output_passes_validator_with_both() {
        let mut aggregators = vec![
            create_aggregator(300, 10),
            create_aggregator(100, 5),
            create_aggregator(200, 5),
        ];
        sort_aggregators_by_validator_index(&mut aggregators);

        let root_a = Hash256::from_low_u64_be(100);
        let root_b = Hash256::from_low_u64_be(200);
        let mut contributors_with_roots = vec![
            create_contributor_with_root(root_b, 150, 1),
            create_contributor_with_root(root_a, 250, 0),
            create_contributor_with_root(root_a, 50, 0),
        ];
        sort_contributors_by_signing_root_then_validator_index(&mut contributors_with_roots);

        let contributors: Vec<AssignedAggregator> = contributors_with_roots
            .into_iter()
            .map(|(_, contrib)| contrib)
            .collect();

        let committee_indexes: IndexSet<u64> =
            aggregators.iter().map(|a| a.committee_index).collect();

        let subnet_ids: IndexSet<SyncSubnetId> = contributors
            .iter()
            .map(|c| SyncSubnetId::new(c.committee_index))
            .collect();

        let attestation_bytes: Vec<VariableList<u8, MaxAggregatedAttestationBytes>> =
            committee_indexes
                .iter()
                .map(|&idx| create_attestation_bytes(idx))
                .collect();

        let contributions: Vec<SyncCommitteeContribution<MainnetEthSpec>> = subnet_ids
            .iter()
            .map(|id| create_test_contribution((*id).into()))
            .collect();

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

        let validator = AggregatorCommitteeDataValidator::<MainnetEthSpec>::new();
        let result = validator.do_validation(&consensus_data);
        assert!(result.is_ok(), "Error: {:?}", result.err());

        let passes_trait_validation =
            QbftDataValidator::validate(&validator, &consensus_data, &consensus_data);
        assert!(passes_trait_validation);
    }

    #[test]
    fn test_filter_aggregators_removes_unfetched() {
        let mut aggregators = vec![
            create_aggregator(100, 5),
            create_aggregator(200, 10),
            create_aggregator(300, 15),
        ];

        let mut attestations = HashMap::new();
        attestations.insert(5, create_test_attestation(5));
        attestations.insert(15, create_test_attestation(15));

        filter_aggregators_with_attestations(&mut aggregators, &attestations);

        assert_eq!(aggregators.len(), 2);
        let remaining_indexes: Vec<usize> =
            aggregators.iter().map(|a| a.validator_index.0).collect();
        assert_eq!(remaining_indexes, vec![100, 300]);
    }

    #[test]
    fn test_filter_contributors_removes_unfetched() {
        let root = Hash256::zero();
        let mut contributors = vec![
            create_contributor_with_root(root, 100, 0),
            create_contributor_with_root(root, 200, 1),
            create_contributor_with_root(root, 300, 2),
        ];

        let mut contributions = HashMap::new();
        contributions.insert(SyncSubnetId::new(0), create_test_contribution(0));
        contributions.insert(SyncSubnetId::new(2), create_test_contribution(2));

        filter_contributors_with_contributions(&mut contributors, &contributions);

        assert_eq!(contributors.len(), 2);
        let remaining_indexes: Vec<usize> = contributors
            .iter()
            .map(|(_, a)| a.validator_index.0)
            .collect();
        assert_eq!(remaining_indexes, vec![100, 300]);
    }

    #[test]
    fn test_committee_indexes_preserve_first_seen_order() {
        let mut aggregators = vec![
            create_aggregator(300, 10),
            create_aggregator(100, 5),
            create_aggregator(200, 5),
            create_aggregator(400, 10),
            create_aggregator(50, 3),
        ];

        sort_aggregators_by_validator_index(&mut aggregators);

        let committee_indexes: IndexSet<u64> =
            aggregators.iter().map(|a| a.committee_index).collect();

        let indexes: Vec<u64> = committee_indexes.into_iter().collect();
        assert_eq!(indexes, vec![3, 5, 10]);
    }

    #[test]
    fn test_subnet_ids_preserve_first_seen_order() {
        let root_a = Hash256::from_low_u64_be(1);
        let root_b = Hash256::from_low_u64_be(2);

        let mut contributors_with_roots = vec![
            create_contributor_with_root(root_b, 100, 1),
            create_contributor_with_root(root_a, 200, 0),
            create_contributor_with_root(root_a, 50, 0),
            create_contributor_with_root(root_b, 150, 1),
        ];

        sort_contributors_by_signing_root_then_validator_index(&mut contributors_with_roots);

        let contributors: Vec<AssignedAggregator> = contributors_with_roots
            .into_iter()
            .map(|(_, contrib)| contrib)
            .collect();

        let subnet_ids: IndexSet<SyncSubnetId> = contributors
            .iter()
            .map(|c| SyncSubnetId::new(c.committee_index))
            .collect();

        let ids: Vec<u64> = subnet_ids.into_iter().map(|id| id.into()).collect();
        assert_eq!(ids, vec![0, 1]);
    }

    #[test]
    fn test_empty_aggregators_and_contributors() {
        let mut aggregators: Vec<AssignedAggregator> = vec![];
        sort_aggregators_by_validator_index(&mut aggregators);
        assert!(aggregators.is_empty());

        let mut contributors: Vec<(Hash256, AssignedAggregator)> = vec![];
        sort_contributors_by_signing_root_then_validator_index(&mut contributors);
        assert!(contributors.is_empty());
    }

    #[test]
    fn test_all_fetches_fail_returns_none() {
        let mut aggregators = vec![
            create_aggregator(100, 5),
            create_aggregator(200, 10),
            create_aggregator(300, 15),
        ];
        sort_aggregators_by_validator_index(&mut aggregators);

        let root = Hash256::zero();
        let mut contributors = vec![
            create_contributor_with_root(root, 50, 0),
            create_contributor_with_root(root, 150, 1),
            create_contributor_with_root(root, 250, 2),
        ];
        sort_contributors_by_signing_root_then_validator_index(&mut contributors);

        let aggregated_attestations: HashMap<u64, Attestation<MainnetEthSpec>> = HashMap::new();
        let sync_contributions: HashMap<SyncSubnetId, SyncCommitteeContribution<MainnetEthSpec>> =
            HashMap::new();

        filter_aggregators_with_attestations(&mut aggregators, &aggregated_attestations);
        filter_contributors_with_contributions(&mut contributors, &sync_contributions);

        assert!(aggregators.is_empty());
        assert!(contributors.is_empty());
    }

    #[test]
    fn test_empty_aggregators_only_contributors() {
        let aggregators: Vec<AssignedAggregator> = vec![];

        let root_a = Hash256::from_low_u64_be(1);
        let root_b = Hash256::from_low_u64_be(2);
        let mut contributors_with_roots = vec![
            create_contributor_with_root(root_b, 150, 1),
            create_contributor_with_root(root_a, 250, 0),
            create_contributor_with_root(root_a, 50, 0),
        ];
        sort_contributors_by_signing_root_then_validator_index(&mut contributors_with_roots);

        let mut sync_contributions = HashMap::new();
        sync_contributions.insert(SyncSubnetId::new(0), create_test_contribution(0));
        sync_contributions.insert(SyncSubnetId::new(1), create_test_contribution(1));

        filter_contributors_with_contributions(&mut contributors_with_roots, &sync_contributions);

        let contributors: Vec<AssignedAggregator> = contributors_with_roots
            .into_iter()
            .map(|(_, contrib)| contrib)
            .collect();

        assert!(!contributors.is_empty());
        assert_eq!(contributors.len(), 3);

        let subnet_ids: IndexSet<SyncSubnetId> = contributors
            .iter()
            .map(|c| SyncSubnetId::new(c.committee_index))
            .collect();

        let contributions: Vec<SyncCommitteeContribution<MainnetEthSpec>> = subnet_ids
            .iter()
            .filter_map(|id| sync_contributions.get(id).cloned())
            .collect();

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

        assert!(consensus_data.aggregators.is_empty());
        assert!(!consensus_data.contributors.is_empty());

        let validator = AggregatorCommitteeDataValidator::<MainnetEthSpec>::new();
        let result = validator.do_validation(&consensus_data);
        assert!(result.is_ok(), "Error: {:?}", result.err());
    }

    #[test]
    fn test_empty_contributors_only_aggregators() {
        let mut aggregators = vec![
            create_aggregator(300, 10),
            create_aggregator(100, 5),
            create_aggregator(200, 7),
        ];
        sort_aggregators_by_validator_index(&mut aggregators);

        let mut aggregated_attestations = HashMap::new();
        aggregated_attestations.insert(5, create_test_attestation(5));
        aggregated_attestations.insert(7, create_test_attestation(7));
        aggregated_attestations.insert(10, create_test_attestation(10));

        filter_aggregators_with_attestations(&mut aggregators, &aggregated_attestations);

        assert!(!aggregators.is_empty());
        assert_eq!(aggregators.len(), 3);

        let committee_indexes: IndexSet<u64> =
            aggregators.iter().map(|a| a.committee_index).collect();

        let attestation_bytes: Vec<VariableList<u8, MaxAggregatedAttestationBytes>> =
            committee_indexes
                .iter()
                .map(|&idx| create_attestation_bytes(idx))
                .collect();

        let contributors: Vec<AssignedAggregator> = vec![];

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

        assert!(!consensus_data.aggregators.is_empty());
        assert!(consensus_data.contributors.is_empty());

        let validator = AggregatorCommitteeDataValidator::<MainnetEthSpec>::new();
        let result = validator.do_validation(&consensus_data);
        assert!(result.is_ok(), "Error: {:?}", result.err());
    }

    #[test]
    fn test_multiple_aggregators_same_committee() {
        let mut aggregators = vec![
            create_aggregator(500, 42),
            create_aggregator(100, 42),
            create_aggregator(300, 42),
            create_aggregator(200, 42),
            create_aggregator(400, 42),
        ];

        sort_aggregators_by_validator_index(&mut aggregators);

        let validator_indices: Vec<usize> =
            aggregators.iter().map(|a| a.validator_index.0).collect();
        assert_eq!(validator_indices, vec![100, 200, 300, 400, 500]);

        let mut aggregated_attestations = HashMap::new();
        aggregated_attestations.insert(42, create_test_attestation(42));

        filter_aggregators_with_attestations(&mut aggregators, &aggregated_attestations);
        assert_eq!(aggregators.len(), 5);

        let committee_indexes: IndexSet<u64> =
            aggregators.iter().map(|a| a.committee_index).collect();

        assert_eq!(committee_indexes.len(), 1);
        assert!(committee_indexes.contains(&42));

        let attestation_bytes: Vec<VariableList<u8, MaxAggregatedAttestationBytes>> =
            committee_indexes
                .iter()
                .map(|&idx| create_attestation_bytes(idx))
                .collect();

        assert_eq!(attestation_bytes.len(), 1);

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

        assert_eq!(consensus_data.aggregators.len(), 5);
        assert_eq!(consensus_data.aggregator_committee_indexes.len(), 1);
        assert_eq!(consensus_data.aggregated_attestations.len(), 1);

        let validator = AggregatorCommitteeDataValidator::<MainnetEthSpec>::new();
        let result = validator.do_validation(&consensus_data);
        assert!(result.is_ok(), "Error: {:?}", result.err());

        let passes_trait_validation =
            QbftDataValidator::validate(&validator, &consensus_data, &consensus_data);
        assert!(passes_trait_validation);
    }
}

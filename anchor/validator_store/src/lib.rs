pub mod aggregator_consensus_builder;
pub mod duty_input_publisher;
mod metrics;
pub mod registration_service;

/// Backwards compatibility: re-export DutyInputPublisher as MetadataService
#[deprecated(
    since = "0.1.0",
    note = "Use duty_input_publisher::DutyInputPublisher instead"
)]
pub mod metadata_service {
    pub use crate::duty_input_publisher::DutyInputPublisher as MetadataService;
}

use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    future::Future,
    num::NonZeroUsize,
    str::from_utf8,
    sync::{Arc, LazyLock},
    time::Duration,
};

use bls::{PublicKeyBytes, SecretKey, Signature};
use database::{NetworkDatabase, NonUniqueIndex, UniqueIndex};
use eth2::types::{BlockContents, FullBlockContents, PublishBlockRequest};
use fork::{Fork, ForkSchedule};
use futures::future::join_all;
use lru::LruCache;
use openssl::{
    pkey::Private,
    rsa::{Padding, Rsa},
};
use parking_lot::Mutex;
use qbft::Completed;
use qbft_manager::{
    AggregatorCommitteeInstanceId, CommitteeInstanceId, ProposerInstanceId, QbftError, QbftManager,
    TimeoutMode, ValidatorDutyKind,
};
use safe_arith::{ArithError, SafeArith};
use signature_collector::{
    CollectionError, SignatureCollectorManager, SignatureMetadata, SignatureRequester,
    ValidatorSigningData,
};
use slashing_protection::{CheckSlashability, NotSafe, Safe, SlashingDatabase};
use slot_clock::SlotClock;
use ssv_types::{
    Cluster, ClusterId, CommitteeId, ENCRYPTED_KEY_LENGTH, ValidatorIndex, ValidatorMetadata,
    consensus::{
        AggregatorCommitteeConsensusData, AggregatorCommitteeDataValidator, BEACON_ROLE_AGGREGATOR,
        BEACON_ROLE_PROPOSER, BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION, BeaconVote,
        BeaconVoteValidator, Contribution, ContributionWrapper, Contributions, DataVersion,
        ProposerConsensusData, ProposerConsensusDataValidator, QbftData, SelectionProofBatchId,
        ValidatorDuty,
    },
    msgid::Role,
    partial_sig::PartialSignatureKind,
    try_to_variable_list,
};
use ssz::{Decode, DecodeError, Encode};
use task_executor::TaskExecutor;
use tokio::{
    select,
    sync::{Barrier, RwLock, watch},
    time::{Instant, sleep},
};
use tracing::{debug, error, info, trace, warn};
use types::{
    AbstractExecPayload, Address, AggregateAndProof, AggregateAndProofBase,
    AggregateAndProofElectra, Attestation, AttestationBase, AttestationElectra, BeaconBlock,
    BeaconBlockRef, BlindedBeaconBlock, BlindedPayload, ChainSpec, ContributionAndProof, Domain,
    Epoch, EthSpec, ForkName, FullPayload, Graffiti, Hash256, SelectionProof,
    SignedAggregateAndProof, SignedBeaconBlock, SignedBlindedBeaconBlock,
    SignedContributionAndProof, SignedRoot, SignedValidatorRegistrationData, SignedVoluntaryExit,
    Slot, SlotData, SyncAggregatorSelectionData, SyncCommitteeContribution, SyncCommitteeMessage,
    SyncSelectionProof, SyncSubnetId, ValidatorRegistrationData, VoluntaryExit,
};
use validator_metrics::IntCounterVec;
use validator_store::{
    DoppelgangerStatus, Error as ValidatorStoreError, ProposalData, SignedBlock, UnsignedBlock,
    ValidatorStore,
};

/// Number of epochs of slashing protection history to keep.
///
/// This acts as a maximum safe-guard against clock drift.
const SLASHING_PROTECTION_HISTORY_EPOCHS: u64 = 512;

const MAX_VALIDATORS_PER_OPERATOR: NonZeroUsize =
    NonZeroUsize::new(3000).expect("3000 is non-zero");

const RANDAO_REVEAL_LOG_NAME: &str = "RANDAO reveal";
const BLOCK_LOG_NAME: &str = "block";
const VALIDATOR_REGISTRATION_LOG_NAME: &str = "validator registration";
const AGGREGATE_LOG_NAME: &str = "aggregate";
const SELECTION_PROOF_LOG_NAME: &str = "selection proof";
const SYNC_SELECTION_PROOF_LOG_NAME: &str = "sync selection proof";
const SYNC_COMMITTEE_SIGNATURE_LOG_NAME: &str = "sync committee signature";
const SYNC_COMMITTEE_CONTRIBUTION_LOG_NAME: &str = "sync committee contribution";

pub struct AnchorValidatorStore<T: SlotClock + 'static, E: EthSpec> {
    database: Arc<NetworkDatabase>,
    decrypted_keys: Mutex<LruCache<[u8; ENCRYPTED_KEY_LENGTH], SecretKey>>,
    signature_collector: Arc<SignatureCollectorManager<T>>,
    qbft_manager: Arc<QbftManager<E, T>>,
    slashing_protection: Arc<SlashingDatabase>,
    slashing_protection_last_prune: Mutex<Epoch>,
    disable_slashing_protection: bool,
    slot_clock: T,
    spec: Arc<ChainSpec>,
    genesis_validators_root: Hash256,
    private_key: Option<Rsa<Private>>,
    fork_schedule: Arc<ForkSchedule>,
    voting_context_tx: watch::Sender<Option<Arc<VotingContext>>>,
    /// Watch channel for `VotingAssignments` (cached at slot start)
    voting_assignments_tx: watch::Sender<Option<Arc<VotingAssignments>>>,
    /// Watch channel for `AggregationAssignments` (cached at 2/3 slot)
    aggregation_assignments_tx: watch::Sender<Option<Arc<AggregationAssignments<E>>>>,
    gas_limit: u64,
    // MEV configuration is applied at the operator level and applies to all validators this
    // operator controls
    builder_boost_factor: Option<u64>,
    prefer_builder_proposals: bool,
    strict_mfp: bool,
    is_synced: watch::Receiver<bool>,
    task_executor: TaskExecutor,
}

impl<T: SlotClock, E: EthSpec> AnchorValidatorStore<T, E> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        database: Arc<NetworkDatabase>,
        signature_collector: Arc<SignatureCollectorManager<T>>,
        qbft_manager: Arc<QbftManager<E, T>>,
        slashing_protection: Arc<SlashingDatabase>,
        disable_slashing_protection: bool,
        slot_clock: T,
        spec: Arc<ChainSpec>,
        genesis_validators_root: Hash256,
        private_key: Option<Rsa<Private>>,
        fork_schedule: Arc<ForkSchedule>,
        gas_limit: u64,
        builder_boost_factor: Option<u64>,
        prefer_builder_proposals: bool,
        strict_mfp: bool,
        is_synced: watch::Receiver<bool>,
        task_executor: TaskExecutor,
    ) -> Arc<AnchorValidatorStore<T, E>> {
        Arc::new(Self {
            database,
            decrypted_keys: Mutex::new(LruCache::new(MAX_VALIDATORS_PER_OPERATOR)),
            signature_collector,
            qbft_manager,
            slashing_protection,
            slashing_protection_last_prune: Mutex::new(Epoch::new(0)),
            disable_slashing_protection,
            slot_clock,
            spec,
            genesis_validators_root,
            private_key,
            fork_schedule,
            voting_context_tx: watch::channel(None).0,
            voting_assignments_tx: watch::channel(None).0,
            aggregation_assignments_tx: watch::channel(None).0,
            gas_limit,
            builder_boost_factor,
            prefer_builder_proposals,
            strict_mfp,
            is_synced,
            task_executor,
        })
    }

    fn get_validator_and_cluster(
        &self,
        validator_pubkey: PublicKeyBytes,
    ) -> Result<(ValidatorMetadata, Cluster), Error> {
        let state = self.database.state();
        let validator = state
            .metadata()
            .get_by(&validator_pubkey)
            .ok_or(Error::UnknownPubkey(validator_pubkey))?
            .clone();

        // First, attempt to get the cluster normally
        if let Some(cluster) = state.clusters().get_by(&validator.cluster_id) {
            if cluster.liquidated {
                return Err(Error::SpecificError(SpecificError::ClusterLiquidated));
            }
            return Ok((validator, cluster.clone()));
        }

        // If cluster is missing, this indicates a database inconsistency
        // Log the error with context
        error!(
            validator_pubkey = %validator_pubkey,
            cluster_id = ?validator.cluster_id,
            "Database inconsistency detected: validator references non-existent cluster"
        );

        // Return specific error with context for potential recovery
        Err(Error::SpecificError(
            SpecificError::ValidatorClusterMismatch {
                validator_pubkey,
                cluster_id: validator.cluster_id,
            },
        ))
    }

    fn get_domain(&self, epoch: Epoch, domain: Domain) -> Hash256 {
        self.spec.get_domain(
            epoch,
            domain,
            &self.spec.fork_at_epoch(epoch),
            self.genesis_validators_root,
        )
    }

    /// Get the set of validator indices for validators we have shares for in a committee.
    ///
    /// This is used to filter decided consensus data to only validators we can sign for.
    fn get_committee_validator_indices(
        &self,
        committee_id: &CommitteeId,
    ) -> HashSet<ValidatorIndex> {
        let state = self.database.state();
        state
            .metadata()
            .get_all_by(committee_id)
            .filter_map(|v| v.index)
            .collect()
    }

    /// Compute the signing root for a sync committee selection proof.
    ///
    /// Each subnet has a different signing root based on `SyncAggregatorSelectionData{Slot,
    /// SubcommitteeIndex}`.
    pub fn compute_sync_selection_root(&self, slot: Slot, subnet_id: u64) -> Hash256 {
        let epoch = slot.epoch(E::slots_per_epoch());
        let domain = self.get_domain(epoch, Domain::SyncCommitteeSelectionProof);
        SyncAggregatorSelectionData {
            slot,
            subcommittee_index: subnet_id,
        }
        .signing_root(domain)
    }

    #[allow(clippy::too_many_arguments)]
    async fn collect_signature(
        &self,
        signature_kind: PartialSignatureKind,
        role: Role,
        collection_mode: CollectionMode,
        validator: &ValidatorMetadata,
        cluster: &Cluster,
        signing_root: Hash256,
        slot: Slot,
    ) -> Result<Signature, Error> {
        let committee_id = cluster.committee_id();
        let metadata = SignatureMetadata {
            kind: signature_kind,
            role,
            threshold: cluster
                .get_f()
                .safe_mul(2)
                .and_then(|x| x.safe_add(1))
                .map_err(SpecificError::from)?,
            slot,
            committee_id,
        };

        let (requester, encrypted_private_key) = {
            let state = self.database.state();
            let requester = match collection_mode {
                CollectionMode::SingleValidator => SignatureRequester::SingleValidator {
                    pubkey: validator.public_key,
                },
                CollectionMode::Committee {
                    num_signatures_to_collect,
                    base_hash,
                } => SignatureRequester::Committee {
                    num_signatures_to_collect,
                    base_hash,
                },
            };
            let encrypted_private_key = state
                .shares()
                .get_by(&validator.public_key)
                .ok_or(Error::UnknownPubkey(validator.public_key))?
                .encrypted_private_key;
            (requester, encrypted_private_key)
        };

        let decrypted_key_share = if let Some(operator_key) = &self.private_key {
            let key = self
                .decrypted_keys
                .lock()
                .try_get_or_insert(encrypted_private_key, || {
                    decrypt_key_share(operator_key, encrypted_private_key, validator.public_key)
                        .map_err(|_| SpecificError::KeyShareDecryptionFailed)
                })
                .cloned()?;
            Some(key)
        } else {
            // We are in imposter mode and cannot decrypt the share.
            None
        };

        let signing_data = ValidatorSigningData {
            root: signing_root,
            index: validator.index.ok_or(SpecificError::MissingIndex)?,
            share: decrypted_key_share,
        };

        let _timer =
            validator_metrics::start_timer_vec(&validator_metrics::SIGNING_TIMES, &["ssv"]);

        let collector =
            self.signature_collector
                .sign_and_collect(metadata, requester, signing_data);
        Ok((*collector.await.map_err(SpecificError::from)?).clone())
    }

    async fn decide_abstract_block(
        &self,
        validator: &ValidatorMetadata,
        cluster: &Cluster,
        signable_block: &impl SignableBlock<E>,
    ) -> Result<UnsignedBlock<E>, Error> {
        let block = signable_block.as_block();
        let slot = block.slot();

        // first, we have to get to consensus
        let timer = metrics::start_timer_vec(&metrics::CONSENSUS_TIMES, &[metrics::BLOCK]);
        let timeout_mode = TimeoutMode::Relative {
            current_round_start_time: self.get_instant_in_slot(slot, Duration::ZERO)?,
        };

        // Define the proposer instance identity for QBFT consensus
        let instance_id = ProposerInstanceId {
            validator: validator.public_key,
            duty: ValidatorDutyKind::Proposal,
            instance_height: slot.as_usize().into(),
        };

        // Get the validator index, ensuring it exists
        let validator_index = validator.index.ok_or(SpecificError::MissingIndex)?;

        // Determine the appropriate version based on block type
        let block_version = block.fork_name_unchecked().into();

        // Create the validator duty information
        let validator_duty = ValidatorDuty {
            r#type: BEACON_ROLE_PROPOSER,
            pub_key: validator.public_key,
            slot,
            validator_index,
            committee_index: 0,
            committee_length: 0,
            committees_at_slot: 0,
            validator_committee_index: 0,
            validator_sync_committee_indices: Default::default(),
        };

        // Package the consensus data
        let consensus_data = ProposerConsensusData {
            duty: validator_duty,
            version: block_version,
            data_ssz: try_to_variable_list(signable_block.as_ssz_bytes(), |provided, max| {
                Error::SpecificError(SpecificError::DataTooLarge(format!(
                    "Block data too large for consensus: {} > {}",
                    provided, max
                )))
            })?,
        };

        let data_validator = self.create_proposer_consensus_data_validator(validator.public_key);

        // Initiate QBFT consensus for this block proposal
        let completed = self
            .qbft_manager
            .decide_instance(
                instance_id,
                consensus_data,
                data_validator,
                timeout_mode,
                cluster,
            )
            .await
            .map_err(SpecificError::from)?;
        drop(timer);

        let completed_data = match completed {
            Completed::TimedOut => return Err(Error::SpecificError(SpecificError::Timeout)),
            Completed::Success(data) => data,
        };

        let fork = ForkName::from(completed_data.version);

        BlindedBeaconBlock::from_ssz_bytes_for_fork(&completed_data.data_ssz, fork)
            .map(UnsignedBlock::Blinded)
            .or_else(|_| {
                FullBlockContents::from_ssz_bytes_for_fork(&completed_data.data_ssz, fork)
                    .map(UnsignedBlock::Full)
            })
            .map_err(|err| Error::SpecificError(SpecificError::InvalidQbftData(err)))
    }

    async fn sign_abstract_block(
        &self,
        validator: &ValidatorMetadata,
        cluster: &Cluster,
        signable_block: impl SignableBlock<E>,
        current_slot: Slot,
    ) -> Result<SignedBlock<E>, Error> {
        debug!(signable_block = ?signable_block.as_block().block_header(), "Decided on BeaconBlock to sign");

        let block = signable_block.as_block();

        // Make sure the block slot is not higher than the current slot to avoid potential attacks.
        if block.slot() > current_slot {
            warn!(
                block_slot = block.slot().as_u64(),
                current_slot = current_slot.as_u64(),
                "Not signing block with slot greater than current slot",
            );
            return Err(Error::GreaterThanCurrentSlot {
                slot: block.slot(),
                current_slot,
            });
        }

        let domain_hash = self.get_domain(block.epoch(), Domain::BeaconProposer);

        let header = block.block_header();

        if !self.disable_slashing_protection {
            convert_slashing_result(self.slashing_protection.check_and_insert_block_proposal(
                &validator.public_key,
                &header,
                domain_hash,
            ))?;
        }

        let signing_root = block.signing_root(domain_hash);
        let signature = self
            .collect_signature(
                PartialSignatureKind::PostConsensus,
                Role::Proposer,
                CollectionMode::SingleValidator,
                validator,
                cluster,
                signing_root,
                header.slot,
            )
            .await?;
        Ok(signable_block.to_signed_block(signature))
    }

    /// Get the [`VotingContext`] for the given [`Slot`], waiting for it to become available if
    /// necessary. If the requested slot has already passed, an error is returned.
    ///
    /// IMPORTANT: The voting context is computed starting at 1/3rd into the slot - so do not try
    /// to retrieve it if sleeping until then is not tolerable.
    async fn get_voting_context(&self, slot: Slot) -> Result<Arc<VotingContext>, Error> {
        let Some(metadata) = self
            .voting_context_tx
            .subscribe()
            .wait_for(|m| {
                m.as_ref()
                    .is_some_and(|metadata| metadata.voting_assignments.slot >= slot)
            })
            .await
            .ok()
            .and_then(|metadata| metadata.clone())
        else {
            error!(%slot, "Unexpected error while waiting for metadata");
            return Err(Error::SpecificError(SpecificError::Metadata));
        };

        if metadata.voting_assignments.slot == slot {
            Ok(metadata)
        } else {
            error!("Got newer metadata - performance issues?");
            Err(Error::SpecificError(SpecificError::Metadata))
        }
    }

    fn update_voting_context(&self, metadata: VotingContext) {
        self.voting_context_tx
            .send_replace(Some(Arc::new(metadata)));
    }

    /// Get validator voting assignments, waiting if not yet available for this slot.
    ///
    /// This method waits until `VotingAssignments` for the requested slot becomes available.
    /// Returns an error if the requested slot has already passed or if the watch channel is closed.
    pub async fn get_voting_assignments(
        &self,
        slot: Slot,
    ) -> Result<Arc<VotingAssignments>, Error> {
        let Some(voting_assignments) = self
            .voting_assignments_tx
            .subscribe()
            .wait_for(|d| d.as_ref().is_some_and(|info| info.slot >= slot))
            .await
            .ok()
            .and_then(|info| info.clone())
        else {
            return Err(Error::SpecificError(SpecificError::MetadataChannelClosed));
        };

        if voting_assignments.slot == slot {
            Ok(voting_assignments)
        } else {
            Err(Error::SpecificError(SpecificError::MetadataSlotPassed))
        }
    }

    /// Update validator voting assignments (called by `MetadataService` at slot start).
    ///
    /// This publishes the `VotingAssignments` to all subscribers via the watch channel.
    pub fn update_voting_assignments(&self, voting_assignments: VotingAssignments) {
        self.voting_assignments_tx
            .send_replace(Some(Arc::new(voting_assignments)));
    }

    /// Get aggregator voting assignments, waiting if not yet available for this slot.
    ///
    /// This method waits until `AggregationAssignments` for the requested slot becomes available.
    /// Called by `produce_signed_aggregate_and_proof` and `produce_signed_contribution_and_proof`
    /// at 2/3 slot.
    ///
    /// Returns an error if the requested slot has already passed or if the watch channel is closed.
    pub async fn get_aggregation_assignments(
        &self,
        slot: Slot,
    ) -> Result<Arc<AggregationAssignments<E>>, Error> {
        let Some(aggregator_info) = self
            .aggregation_assignments_tx
            .subscribe()
            .wait_for(|a| a.as_ref().is_some_and(|info| info.slot >= slot))
            .await
            .ok()
            .and_then(|info| info.clone())
        else {
            return Err(Error::SpecificError(
                SpecificError::AggregatorInfoChannelClosed,
            ));
        };

        if aggregator_info.slot == slot {
            Ok(aggregator_info)
        } else {
            Err(Error::SpecificError(
                SpecificError::AggregatorInfoSlotPassed,
            ))
        }
    }

    /// Update aggregator voting assignments (called by `MetadataService` Phase 3 at 2/3 slot).
    ///
    /// This publishes the `AggregationAssignments` to all subscribers via the watch channel.
    /// At 2/3 slot, selection proofs have been computed by Lighthouse, so
    /// `DutyAndProof.selection_proof.is_some()` accurately indicates `is_aggregator`.
    pub fn update_aggregation_assignments(&self, info: AggregationAssignments<E>) {
        self.aggregation_assignments_tx
            .send_replace(Some(Arc::new(info)));
    }

    /// Return [`SpecificError::Timeout`] if the given future does not complete at `delay` into the
    /// given slot.
    ///
    /// In the unlikely case the `slot_clock` errors, we time out after `delay`.
    async fn timeout_within_slot<O>(
        &self,
        slot: Slot,
        delay: Duration,
        future: impl Future<Output = Result<O, impl Into<Error>>>,
    ) -> Result<O, Error> {
        let timeout_time = self
            .slot_clock
            .start_of(slot)
            .and_then(|start| {
                self.slot_clock
                    .now_duration()
                    .map(|now| (start + delay).saturating_sub(now))
            })
            .unwrap_or(delay);

        select! {
            result = future => {
                result.map_err(Into::into)
            },
            _ = sleep(timeout_time) => {
                Err(SpecificError::Timeout.into())
            }
        }
    }

    fn get_instant_in_slot(&self, slot: Slot, delay: Duration) -> Result<Instant, Error> {
        // We can calculate an instant only by adding a duration to the current instant.

        // First, we get the duration since unix epoch to the target time.
        let target_duration = self
            .slot_clock
            .start_of(slot)
            .map(|start| start + delay)
            .ok_or(SpecificError::SlotClock)?;
        // Then, we get the current time as duration since unix epoch.
        let now_duration = self
            .slot_clock
            .now_duration()
            .ok_or(SpecificError::SlotClock)?;
        // We calculate the difference and add or substract it depending on whether the target is
        // before or after the current time.
        let difference = target_duration.abs_diff(now_duration);
        let instant = if target_duration > now_duration {
            Instant::now() + difference
        } else {
            Instant::now() - difference
        };
        Ok(instant)
    }

    pub async fn collect_voluntary_exit_partial_signatures(
        &self,
        validator_pubkey: PublicKeyBytes,
        voluntary_exit: VoluntaryExit,
        slot: Slot,
    ) -> Result<SignedVoluntaryExit, Error> {
        let spec = self.spec.clone();
        let domain_hash = voluntary_exit.get_domain(self.genesis_validators_root, &spec);
        let signing_root = voluntary_exit.signing_root(domain_hash);
        let (validator, cluster) = self.get_validator_and_cluster(validator_pubkey)?;

        let signature = self
            .collect_signature(
                PartialSignatureKind::VoluntaryExit,
                Role::VoluntaryExit,
                CollectionMode::SingleValidator,
                &validator,
                &cluster,
                signing_root,
                slot,
            )
            .await?;

        // Create signed exit message
        let signed_exit = SignedVoluntaryExit {
            message: voluntary_exit,
            signature,
        };

        Ok(signed_exit)
    }

    fn create_proposer_consensus_data_validator(
        &self,
        validator_pubkey: PublicKeyBytes,
    ) -> Box<ProposerConsensusDataValidator<E>> {
        Box::new(ProposerConsensusDataValidator::new(
            Arc::clone(&self.slashing_protection),
            self.disable_slashing_protection,
            self.spec.clone(),
            validator_pubkey,
            self.genesis_validators_root,
        ))
    }

    fn create_beacon_vote_validator(
        &self,
        slot: Slot,
        validator_attestation_committees: HashMap<PublicKeyBytes, u64>,
    ) -> Box<BeaconVoteValidator<E>> {
        let slashing_protection =
            (!self.disable_slashing_protection).then(|| Arc::clone(&self.slashing_protection));

        Box::new(BeaconVoteValidator::new(
            slot,
            slashing_protection,
            self.spec.clone(),
            validator_attestation_committees,
            self.genesis_validators_root,
            self.strict_mfp,
        ))
    }

    fn get_attesting_validators_in_committee(
        &self,
        metadata: &VotingContext,
        committee_id: CommitteeId,
    ) -> HashMap<PublicKeyBytes, u64> {
        let committee_validators = self
            .database
            .state()
            .metadata()
            .get_all_by(&committee_id)
            .map(|v| v.public_key)
            .collect::<HashSet<_>>();

        metadata
            .voting_assignments
            .attesting_committees
            .iter()
            .filter_map(|(&pubkey, &index)| {
                committee_validators
                    .contains(&pubkey)
                    .then_some((pubkey, index))
            })
            .collect::<HashMap<_, _>>()
    }

    /// Boole+ path for `produce_signed_aggregate_and_proof`: committee-based consensus using
    /// `AggregatorCommitteeConsensusData`.
    #[allow(clippy::too_many_arguments)]
    async fn produce_signed_aggregate_and_proof_boole(
        &self,
        validator_pubkey: PublicKeyBytes,
        aggregator_index: u64,
        signing_epoch: Epoch,
        slot: Slot,
        validator: ValidatorMetadata,
        cluster: Cluster,
        committee_id: CommitteeId,
    ) -> Result<SignedAggregateAndProof<E>, Error> {
        // Get pre-built consensus data from AggregationAssignments (waits until 2/3 slot)
        let aggregation_assignments = self.get_aggregation_assignments(slot).await?;

        // Get pre-built consensus data for this committee
        let our_consensus_data = aggregation_assignments
            .get_consensus_data(&committee_id)
            .ok_or(Error::SpecificError(SpecificError::ConsensusDataNotFound))?;

        // Run QBFT consensus with committee-based instance ID
        let timer =
            metrics::start_timer_vec(&metrics::CONSENSUS_TIMES, &[metrics::AGGREGATE_AND_PROOF]);
        let timeout_mode = TimeoutMode::SlotTime {
            instance_start_time: self.get_instant_in_slot(
                slot,
                Duration::from_secs(self.spec.seconds_per_slot) * 2 / 3,
            )?,
        };

        let completed = self
            .qbft_manager
            .decide_instance(
                AggregatorCommitteeInstanceId {
                    committee: committee_id,
                    instance_height: slot.as_usize().into(),
                },
                (*our_consensus_data).clone(),
                Box::new(AggregatorCommitteeDataValidator::new()),
                timeout_mode,
                &cluster,
            )
            .await
            .map_err(SpecificError::from)?;
        drop(timer);

        let decided_data = match completed {
            Completed::TimedOut => {
                return Err(Error::SpecificError(SpecificError::Timeout));
            }
            Completed::Success(data) => data,
        };

        // First check if this validator is in the decided data at all.
        // If the proposer's beacon API failed to fetch this validator's aggregate,
        // or if the proposer had a different view, this validator won't be present.
        let validator_index = validator.index.ok_or(SpecificError::MissingIndex)?;

        let Some(decided_aggregator) = decided_data
            .aggregators
            .iter()
            .find(|agg| agg.validator_index == validator_index)
        else {
            // This validator is not in the decided data, meaning either:
            // 1. The proposer's beacon API failed to fetch this validator's aggregate
            // 2. The proposer had a different view of aggregator duties
            // We skip this validator gracefully as it won't be part of the final signature
            debug!(
                ?validator_index,
                ?validator_pubkey,
                "Validator not in decided data - skipping (proposer had different view or API failure)"
            );
            return Err(Error::SpecificError(
                SpecificError::ValidatorNotInConsensus(validator_index),
            ));
        };

        // Now find the committee_index for this validator from the decided data
        // The decided_aggregator tells us which committee this validator is aggregating for
        let committee_index = decided_aggregator.committee_index;

        // Find the position of this committee in the decided data
        let Some(decided_aggregate_idx) = decided_data
            .aggregator_committee_indexes
            .iter()
            .position(|&idx| idx == committee_index)
        else {
            // This should not happen if decided_data is internally consistent
            warn!(
                ?committee_index,
                ?validator_index,
                "Committee index from aggregator not found in decided data - data inconsistency"
            );
            return Err(Error::SpecificError(
                SpecificError::AggregateNotInConsensus(committee_index),
            ));
        };

        let decided_aggregate_bytes = decided_data
            .aggregated_attestations
            .get(decided_aggregate_idx)
            .ok_or(Error::SpecificError(
                SpecificError::AggregateNotInConsensus(committee_index),
            ))?;

        // Decode based on fork version
        let decided_aggregate = if decided_data.version < DataVersion::from(ForkName::Electra) {
            Attestation::Base(
                AttestationBase::from_ssz_bytes(decided_aggregate_bytes)
                    .map_err(SpecificError::InvalidQbftData)?,
            )
        } else {
            Attestation::Electra(
                AttestationElectra::from_ssz_bytes(decided_aggregate_bytes)
                    .map_err(SpecificError::InvalidQbftData)?,
            )
        };

        // Extract the decided selection proof from the aggregator we already found
        let decided_selection_proof =
            SelectionProof::from(decided_aggregator.selection_proof.clone());

        // Build the AggregateAndProof with decided values
        let message = AggregateAndProof::from_attestation(
            aggregator_index,
            decided_aggregate,
            decided_selection_proof,
        );

        debug!(
            aggregator_index = ?message.aggregator_index(),
            data = ?message.aggregate().data(),
            num_set_aggregation_bits = message.aggregate().num_set_aggregation_bits(),
            "Decided on AggregateAndProof to sign (Boole+ committee consensus)"
        );

        // Calculate signature count for post-consensus committee collection
        let committee_validator_indices = self.get_committee_validator_indices(&committee_id);

        // Count all post-consensus signatures (aggregators + contributors) in decided data
        // that we have shares for. This uses the combined count to ensure all signatures
        // are batched into a single message, avoiding validation failures from split
        // messages. The count uses `decided_data` (QBFT consensus result)
        // filtered by our shares, rather than local views, to correctly
        // handle divergent operator views.
        let num_signatures_to_collect = decided_data
            .post_consensus_signature_count(|idx| committee_validator_indices.contains(idx));

        let data_hash = decided_data.hash();

        let domain_hash = self.get_domain(signing_epoch, Domain::AggregateAndProof);
        let signing_root = message.signing_root(domain_hash);

        let signature = self
            .collect_signature(
                PartialSignatureKind::PostConsensus,
                Role::AggregatorCommittee,
                CollectionMode::Committee {
                    num_signatures_to_collect,
                    base_hash: data_hash,
                },
                &validator,
                &cluster,
                signing_root,
                slot,
            )
            .await?;

        Ok(SignedAggregateAndProof::from_aggregate_and_proof(
            message, signature,
        ))
    }

    /// Pre-Boole path for `produce_signed_aggregate_and_proof`: per-validator consensus.
    #[allow(clippy::too_many_arguments)]
    async fn produce_signed_aggregate_and_proof_alan(
        &self,
        validator_pubkey: PublicKeyBytes,
        aggregator_index: u64,
        aggregate: Attestation<E>,
        selection_proof: SelectionProof,
        signing_epoch: Epoch,
        validator: ValidatorMetadata,
        cluster: Cluster,
    ) -> Result<SignedAggregateAndProof<E>, Error> {
        let version = match &aggregate {
            Attestation::Base(_) => ForkName::Base.into(),
            Attestation::Electra(_) => ForkName::Electra.into(),
        };

        let message =
            AggregateAndProof::from_attestation(aggregator_index, aggregate, selection_proof);

        // first, we have to get to consensus
        let timer =
            metrics::start_timer_vec(&metrics::CONSENSUS_TIMES, &[metrics::AGGREGATE_AND_PROOF]);
        let timeout_mode = TimeoutMode::SlotTime {
            instance_start_time: self.get_instant_in_slot(
                message.aggregate().data().slot,
                Duration::from_secs(self.spec.seconds_per_slot) * 2 / 3,
            )?,
        };

        let completed = self
            .qbft_manager
            .decide_instance(
                ProposerInstanceId {
                    validator: validator_pubkey,
                    duty: ValidatorDutyKind::Aggregator,
                    instance_height: message.aggregate().data().slot.as_usize().into(),
                },
                ProposerConsensusData {
                    duty: ValidatorDuty {
                        r#type: BEACON_ROLE_AGGREGATOR,
                        pub_key: validator_pubkey,
                        slot: message.aggregate().data().slot,
                        validator_index: validator.index.ok_or(SpecificError::MissingIndex)?,
                        committee_index: message.aggregate().data().index,
                        // TODO: it seems the below are not needed (anymore?)
                        // potentially related: https://github.com/sigp/anchor/issues/263
                        committee_length: 0,
                        committees_at_slot: 0,
                        validator_committee_index: 0,
                        validator_sync_committee_indices: Default::default(),
                    },
                    version,
                    data_ssz: try_to_variable_list(message.as_ssz_bytes(), |provided, max| {
                        Error::SpecificError(SpecificError::DataTooLarge(format!(
                            "Attestation data too large for consensus: {} > {}",
                            provided, max
                        )))
                    })?,
                },
                self.create_proposer_consensus_data_validator(validator_pubkey),
                timeout_mode,
                &cluster,
            )
            .await
            .map_err(SpecificError::from)?;
        drop(timer);

        let data = match completed {
            Completed::TimedOut => {
                return Err(Error::SpecificError(SpecificError::Timeout));
            }
            Completed::Success(data) => data,
        };

        let message = if ForkName::from(data.version) < ForkName::Electra {
            AggregateAndProof::Base(
                AggregateAndProofBase::from_ssz_bytes(&data.data_ssz)
                    .map_err(|e| Error::SpecificError(SpecificError::InvalidQbftData(e)))?,
            )
        } else {
            AggregateAndProof::Electra(
                AggregateAndProofElectra::from_ssz_bytes(&data.data_ssz)
                    .map_err(|e| Error::SpecificError(SpecificError::InvalidQbftData(e)))?,
            )
        };

        debug!(
            aggregator_index = ?message.aggregator_index(),
            data = ?message.aggregate().data(),
            num_set_aggregation_bits = message.aggregate().num_set_aggregation_bits(),
            "Decided on AggregateAndProof to sign"
        );

        let domain_hash = self.get_domain(signing_epoch, Domain::AggregateAndProof);
        let signing_root = message.signing_root(domain_hash);
        let signature = self
            .collect_signature(
                PartialSignatureKind::PostConsensus,
                Role::Aggregator,
                CollectionMode::SingleValidator,
                &validator,
                &cluster,
                signing_root,
                message.aggregate().get_slot(),
            )
            .await?;

        Ok(SignedAggregateAndProof::from_aggregate_and_proof(
            message, signature,
        ))
    }

    /// Boole+ path for `produce_signed_contribution_and_proof`: committee-based consensus using
    /// `AggregatorCommitteeConsensusData`.
    #[allow(clippy::too_many_arguments)]
    async fn produce_signed_contribution_and_proof_boole(
        &self,
        aggregator_index: u64,
        aggregator_pubkey: PublicKeyBytes,
        subcommittee_index: u64,
        slot: Slot,
        epoch: Epoch,
        validator: ValidatorMetadata,
        cluster: Cluster,
        committee_id: CommitteeId,
    ) -> Result<SignedContributionAndProof<E>, Error> {
        // Get pre-built consensus data from AggregationAssignments (waits until 2/3 slot)
        let aggregation_assignments = self.get_aggregation_assignments(slot).await?;

        // Get pre-built consensus data for this committee
        let our_consensus_data = aggregation_assignments
            .get_consensus_data(&committee_id)
            .ok_or(Error::SpecificError(SpecificError::ConsensusDataNotFound))?;

        // Run QBFT consensus with committee-based instance ID
        let timer = metrics::start_timer_vec(
            &metrics::CONSENSUS_TIMES,
            &[metrics::SYNC_CONTRIBUTION_AND_PROOF],
        );
        let timeout_mode = TimeoutMode::SlotTime {
            instance_start_time: self.get_instant_in_slot(
                slot,
                Duration::from_secs(self.spec.seconds_per_slot) * 2 / 3,
            )?,
        };

        let completed = self
            .qbft_manager
            .decide_instance(
                AggregatorCommitteeInstanceId {
                    committee: committee_id,
                    instance_height: slot.as_usize().into(),
                },
                (*our_consensus_data).clone(),
                Box::new(AggregatorCommitteeDataValidator::new()),
                timeout_mode,
                &cluster,
            )
            .await
            .map_err(SpecificError::from)?;
        drop(timer);

        let decided_data: AggregatorCommitteeConsensusData<E> = match completed {
            Completed::TimedOut => {
                return Err(Error::SpecificError(SpecificError::Timeout));
            }
            Completed::Success(data) => data,
        };

        // Extract this validator's decided selection proof by matching `validator_index`
        // AND `subcommittee_index`. Unlike attestation aggregators where each validator
        // has one entry, sync contributors may have multiple entries (one per
        // subcommittee). If this validator+subcommittee is not in the
        // decided data (e.g., another operator's proposal won with fewer
        // validators), we return `ValidatorNotInConsensus` early. This
        // prevents us from creating a post-consensus partial signature for a
        // validator that won't be counted in `num_signatures_to_collect`, which would cause
        // the signature collector to wait forever for signatures that will never arrive.
        let validator_index = validator.index.ok_or(SpecificError::MissingIndex)?;

        let Some(decided_contributor) = decided_data.contributors.iter().find(|contrib| {
            contrib.validator_index == validator_index
                && contrib.committee_index == subcommittee_index
        }) else {
            debug!(
                ?validator_index,
                subcommittee_index,
                "Validator not in decided data, skipping due to divergent operator views"
            );
            return Err(Error::SpecificError(
                SpecificError::ValidatorNotInConsensus(validator_index),
            ));
        };

        // Look up the actual contribution from decided data by subcommittee_index
        let Some(decided_contribution) = decided_data
            .sync_committee_contributions
            .iter()
            .find(|c| c.subcommittee_index == subcommittee_index)
        else {
            // This can happen when the beacon API failed to return the sync
            // contribution during consensus data building, causing this subcommittee
            // to be filtered out.
            debug!(
                subcommittee_index,
                ?aggregator_pubkey,
                "Contribution not in consensus data - likely filtered due to beacon API failure"
            );
            return Err(Error::SpecificError(
                SpecificError::ContributionNotInConsensus(subcommittee_index),
            ));
        };

        // Build the ContributionAndProof with decided values
        let message = ContributionAndProof {
            aggregator_index,
            contribution: decided_contribution.clone(),
            selection_proof: decided_contributor.selection_proof.clone(),
        };

        debug!(
            aggregator_index = ?message.aggregator_index,
            slot = %message.contribution.slot,
            block_root = ?message.contribution.beacon_block_root,
            subcommittee_index = message.contribution.subcommittee_index,
            num_set_aggregation_bits = message.contribution.aggregation_bits.num_set_bits(),
            "Decided on ContributionAndProof to sign (Boole+ committee consensus)"
        );

        // Calculate signature count for post-consensus committee collection
        let committee_validator_indices = self.get_committee_validator_indices(&committee_id);

        // Count all post-consensus signatures (aggregators + contributors) in decided data
        // that we have shares for. This uses the combined count to ensure all signatures
        // are batched into a single message, avoiding validation failures from split
        // messages. The count uses `decided_data` (QBFT consensus result)
        // filtered by our shares, rather than local views, to correctly
        // handle divergent operator views.
        let num_signatures_to_collect = decided_data
            .post_consensus_signature_count(|idx| committee_validator_indices.contains(idx));

        let data_hash = decided_data.hash();

        let domain_hash = self.get_domain(epoch, Domain::ContributionAndProof);
        let signing_root = message.signing_root(domain_hash);

        let signature = self
            .collect_signature(
                PartialSignatureKind::PostConsensus,
                Role::AggregatorCommittee,
                CollectionMode::Committee {
                    num_signatures_to_collect,
                    base_hash: data_hash,
                },
                &validator,
                &cluster,
                signing_root,
                slot,
            )
            .await?;

        Ok(SignedContributionAndProof { message, signature })
    }

    /// Pre-Boole path for `produce_signed_contribution_and_proof`: per-validator consensus.
    #[allow(clippy::too_many_arguments)]
    async fn produce_signed_contribution_and_proof_alan(
        &self,
        aggregator_index: u64,
        aggregator_pubkey: PublicKeyBytes,
        contribution: SyncCommitteeContribution<E>,
        selection_proof: SyncSelectionProof,
        slot: Slot,
        epoch: Epoch,
        subcommittee_index: u64,
        validator: ValidatorMetadata,
        cluster: Cluster,
    ) -> Result<SignedContributionAndProof<E>, Error> {
        let signing_data = ContributionAndProofSigningData {
            contribution,
            selection_proof,
        };

        // Get aggregator voting assignments from Phase 3 (published at 2/3 slot)
        let aggregator_info = self.get_aggregation_assignments(slot).await?;

        let signing_data = match aggregator_info
            .multi_sync_aggregators
            .get(&aggregator_pubkey)
        {
            None => vec![signing_data],
            Some(contribution_waiter) => {
                let mut data = contribution_waiter.submit_and_wait(signing_data).await;
                data.sort_by(|a, b| {
                    a.contribution
                        .subcommittee_index
                        .cmp(&b.contribution.subcommittee_index)
                });
                data
            }
        };

        let data = Contributions::new(
            signing_data
                .iter()
                .map(|signing_data| {
                    // Wrap contribution to match Go-SSV's encoding
                    ContributionWrapper::from(Contribution {
                        selection_proof_sig: signing_data.selection_proof.clone().into(),
                        contribution: signing_data.contribution.clone(),
                    })
                })
                .collect(),
        )
        .map_err(|_| SpecificError::TooManySyncSubnetsToSign)?;

        let timer = metrics::start_timer_vec(
            &metrics::CONSENSUS_TIMES,
            &[metrics::SYNC_CONTRIBUTION_AND_PROOF],
        );
        let timeout_mode = TimeoutMode::SlotTime {
            instance_start_time: self.get_instant_in_slot(
                slot,
                Duration::from_secs(self.spec.seconds_per_slot) * 2 / 3,
            )?,
        };

        let completed = self
            .qbft_manager
            .decide_instance(
                ProposerInstanceId {
                    validator: aggregator_pubkey,
                    duty: ValidatorDutyKind::SyncCommitteeAggregator,
                    instance_height: slot.as_usize().into(),
                },
                ProposerConsensusData {
                    duty: ValidatorDuty {
                        r#type: BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION,
                        pub_key: aggregator_pubkey,
                        slot,
                        validator_index: validator.index.ok_or(SpecificError::MissingIndex)?,
                        committee_index: 0,
                        committee_length: 0,
                        committees_at_slot: 0,
                        validator_committee_index: aggregator_index,
                        validator_sync_committee_indices: Default::default(),
                    },
                    version: ForkName::Altair.into(),
                    data_ssz: try_to_variable_list(data.as_ssz_bytes(), |provided, max| {
                        Error::SpecificError(SpecificError::DataTooLarge(format!(
                            "Sync committee data too large for consensus: {} > {}",
                            provided, max
                        )))
                    })?,
                },
                self.create_proposer_consensus_data_validator(aggregator_pubkey),
                timeout_mode,
                &cluster,
            )
            .await;
        drop(timer);

        let data = match completed {
            Ok(Completed::Success(data)) => data,
            Ok(Completed::TimedOut) => return Err(SpecificError::Timeout.into()),
            Err(err) => return Err(SpecificError::QbftError(err).into()),
        };

        let data = Contributions::<E>::from_ssz_bytes(&data.data_ssz)
            .map_err(|e| Error::from(SpecificError::InvalidQbftData(e)))?;

        let data = data
            .into_iter()
            .map(Contribution::from)
            .find(|data| data.contribution.subcommittee_index == subcommittee_index)
            .ok_or(SpecificError::NoDataAgreed)?;

        debug!(
            slot = %data.contribution.slot,
            block_root = ?data.contribution.beacon_block_root,
            subcommittee_index = data.contribution.subcommittee_index,
            num_set_aggregation_bits = data.contribution.aggregation_bits.num_set_bits(),
            "Decided on Contribution to sign"
        );

        let domain_hash = self.get_domain(epoch, Domain::ContributionAndProof);
        let message = ContributionAndProof {
            aggregator_index,
            contribution: data.contribution,
            selection_proof: data.selection_proof_sig,
        };
        let signing_root = message.signing_root(domain_hash);
        self.collect_signature(
            PartialSignatureKind::PostConsensus,
            Role::SyncCommittee,
            CollectionMode::SingleValidator,
            &validator,
            &cluster,
            signing_root,
            slot,
        )
        .await
        .map(|signature| SignedContributionAndProof { message, signature })
    }

    async fn sign_attestation(
        &self,
        validator_pubkey: PublicKeyBytes,
        validator_committee_position: usize,
        attestation: &mut Attestation<E>,
        current_epoch: Epoch,
    ) -> Result<(), Error> {
        if !*self.is_synced.borrow() {
            return Err(Error::SpecificError(SpecificError::NotSynced));
        }

        let (validator, cluster) = self.get_validator_and_cluster(validator_pubkey)?;
        let voting_context_tx = self.get_voting_context(attestation.data().slot).await?;

        let validator_attestation_committees =
            self.get_attesting_validators_in_committee(&voting_context_tx, cluster.committee_id());

        let timer = metrics::start_timer_vec(&metrics::CONSENSUS_TIMES, &[metrics::BEACON_VOTE]);
        let timeout_mode = TimeoutMode::SlotTime {
            instance_start_time: self.get_instant_in_slot(
                attestation.data().slot,
                Duration::from_secs(self.spec.seconds_per_slot) / 3,
            )?,
        };
        let completed = self
            .qbft_manager
            .decide_instance(
                CommitteeInstanceId {
                    committee: cluster.committee_id(),
                    instance_height: attestation.data().slot.as_usize().into(),
                },
                BeaconVote {
                    block_root: attestation.data().beacon_block_root,
                    source: attestation.data().source,
                    target: attestation.data().target,
                },
                self.create_beacon_vote_validator(
                    attestation.data().slot,
                    validator_attestation_committees,
                ),
                timeout_mode,
                &cluster,
            )
            .await
            .map_err(SpecificError::from)?;
        drop(timer);

        let data = match completed {
            Completed::TimedOut => return Err(Error::SpecificError(SpecificError::Timeout)),
            Completed::Success(data) => data,
        };
        let data_hash = data.hash();
        attestation.data_mut().beacon_block_root = data.block_root;
        attestation.data_mut().source = data.source;
        attestation.data_mut().target = data.target;

        // yay - we agree! let's sign the att we agreed on
        let domain_hash = self.get_domain(current_epoch, Domain::BeaconAttester);

        // Calculate signature count for post-consensus committee collection
        let committee_validator_indices =
            self.get_committee_validator_indices(&cluster.committee_id());

        // Use `voting_message_count_for_committee` for post-consensus (flat counting)
        let num_signatures_to_collect = voting_context_tx
            .voting_assignments
            .voting_message_count_for_committee(|idx| committee_validator_indices.contains(idx));

        let signing_root = attestation.data().signing_root(domain_hash);
        let signature = self
            .collect_signature(
                PartialSignatureKind::PostConsensus,
                Role::Committee,
                CollectionMode::Committee {
                    num_signatures_to_collect,
                    base_hash: data_hash,
                },
                &validator,
                &cluster,
                signing_root,
                attestation.data().slot,
            )
            .await?;
        attestation
            .add_signature(&signature, validator_committee_position)
            .map_err(Error::UnableToSignAttestation)?;

        Ok(())
    }

    /// Provide slashing protection for attestations, safely updating the slashing protection DB.
    ///
    /// Returns a vec of safe attestations which have passed slashing protection. Unsafe
    /// attestations will be dropped and result in warning logs.
    fn slashing_protection_attestations(
        &self,
        attestations: Vec<(u64, Attestation<E>, PublicKeyBytes)>,
    ) -> Result<Vec<(u64, Attestation<E>)>, Error> {
        let mut safe_attestations = Vec::with_capacity(attestations.len());
        let mut attestations_to_check = Vec::with_capacity(attestations.len());

        for (_, attestation, validator_pubkey) in &attestations {
            let domain_hash =
                self.get_domain(attestation.data().target.epoch, Domain::BeaconAttester);
            attestations_to_check.push((
                attestation.data(),
                validator_pubkey,
                domain_hash,
                if self.disable_slashing_protection {
                    CheckSlashability::No
                } else {
                    CheckSlashability::Yes
                },
            ))
        }

        // Batch check the attestations against the slashing protection DB while preserving the
        // order so we can zip the results against the original vec.
        //
        // If the DB transaction fails then we consider the entire batch slashable and discard it.
        let results: Vec<Result<(), Error>> = self
            .slashing_protection
            .check_and_insert_attestations(&attestations_to_check)
            .map_err(Error::Slashable)?
            .into_iter()
            .map(convert_slashing_result)
            .collect();

        for ((validator_index, attestation, validator_pubkey), slashing_status) in
            attestations.into_iter().zip(results.into_iter())
        {
            match slashing_status {
                Ok(()) => {
                    safe_attestations.push((validator_index, attestation));
                    validator_metrics::inc_counter_vec(
                        &validator_metrics::SIGNED_ATTESTATIONS_TOTAL,
                        &[validator_metrics::SUCCESS],
                    );
                }
                Err(Error::SameData) => {
                    warn!("Skipping previously signed attestation");
                    validator_metrics::inc_counter_vec(
                        &validator_metrics::SIGNED_ATTESTATIONS_TOTAL,
                        &[validator_metrics::SAME_DATA],
                    );
                }
                Err(Error::Slashable(NotSafe::UnregisteredValidator(pk))) => {
                    error!(
                        ?pk,
                        "Internal error: validator was not properly registered for slashing protection",
                    );
                    validator_metrics::inc_counter_vec(
                        &validator_metrics::SIGNED_ATTESTATIONS_TOTAL,
                        &[validator_metrics::UNREGISTERED],
                    );
                }
                Err(Error::Slashable(err)) => {
                    error!(?err, "Not signing slashable attestation");
                    validator_metrics::inc_counter_vec(
                        &validator_metrics::SIGNED_ATTESTATIONS_TOTAL,
                        &[validator_metrics::SLASHABLE],
                    );
                }
                Err(e) => {
                    error!(
                        error = ?e,
                        public_key = ?validator_pubkey,
                        "Unexpected error during slashing protection check"
                    );
                    validator_metrics::inc_counter_vec(
                        &validator_metrics::SIGNED_ATTESTATIONS_TOTAL,
                        &[metrics::OTHER_ERROR],
                    );
                }
            }
        }

        Ok(safe_attestations)
    }
}

/// # Arguments
/// - `log_name`: The name for the object being signed, used in error logging.
/// - `metric`: The metric updated according to the result.
/// - `action`: The future performing the necessary actions and returning the result to check.
async fn run_and_update_metrics<T>(
    log_name: &'static str,
    metric: &LazyLock<validator_metrics::Result<IntCounterVec>>,
    action: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    let result = action.await;
    match &result {
        Ok(_) => {
            validator_metrics::inc_counter_vec(metric, &[validator_metrics::SUCCESS]);
        }
        Err(Error::SameData) => {
            warn!("Skipping signing of previously signed {log_name}",);
            validator_metrics::inc_counter_vec(metric, &[validator_metrics::SAME_DATA]);
        }
        Err(Error::Slashable(NotSafe::UnregisteredValidator(pk))) => {
            error!(
                ?pk,
                "Internal error: validator was not properly registered for slashing protection",
            );
            validator_metrics::inc_counter_vec(metric, &[validator_metrics::UNREGISTERED]);
        }
        Err(Error::Slashable(err)) => {
            error!(?err, "Not signing slashable {log_name}",);
            validator_metrics::inc_counter_vec(metric, &[validator_metrics::SLASHABLE]);
        }
        Err(Error::SpecificError(SpecificError::Timeout)) => {
            warn!("Signing {log_name} timed out - other operators might be offline");
            validator_metrics::inc_counter_vec(metric, &[metrics::TIMEOUT]);
        }
        Err(err) => {
            error!(?err, "Unexpected error while signing {log_name}");
            validator_metrics::inc_counter_vec(metric, &[metrics::OTHER_ERROR]);
        }
    }
    result
}

fn decrypt_key_share(
    operator_key: &Rsa<Private>,
    encrypted_private_key: [u8; ENCRYPTED_KEY_LENGTH],
    pubkey_bytes: PublicKeyBytes,
) -> Result<SecretKey, ()> {
    // the buffer size must be larger than or equal the modulus size
    let mut key_hex = [0; 2048 / 8];
    let length = operator_key
        .private_decrypt(&encrypted_private_key, &mut key_hex, Padding::PKCS1)
        .map_err(|e| error!(?e, validator = %pubkey_bytes, "Share decryption failed"))?;

    let key_hex = from_utf8(&key_hex[..length]).map_err(|err| {
        error!(
            ?err,
            validator = %pubkey_bytes,
            "Share decryption yielded non-utf8 data"
        )
    })?;

    let mut secret_key = [0; 32];
    hex::decode_to_slice(
        key_hex.strip_prefix("0x").unwrap_or(key_hex),
        &mut secret_key,
    )
    .map_err(|err| {
        error!(
            ?err,
            validator = %pubkey_bytes,
            "Decrypted share is not a hex string of size 64"
        )
    })?;

    SecretKey::deserialize(&secret_key)
        .map_err(|err| error!(?err, validator = %pubkey_bytes, "Invalid secret key decrypted"))
}

/// Context for voting duties at 1/3 slot.
///
/// Contains cached voting assignments and the beacon_vote fetched from the beacon node.
pub struct VotingContext {
    /// Cached voting assignments (computed at slot start, reused here)
    pub voting_assignments: Arc<VotingAssignments>,
    /// The `BeaconVote` (only available at 1/3 slot from beacon node)
    pub beacon_vote: BeaconVote,
}

/// Cached validator voting assignments for a slot.
///
/// This struct caches voting assignments computed at slot start and reuses it at 1/3 slot,
/// eliminating redundant computation. It supports two different counting patterns:
///
/// 1. **Committee messages** (attestation + sync): `+1` per sync validator
/// 2. **Selection proofs** (aggregator committee): `+N` per sync validator (N = subnets)
#[derive(Debug, Clone)]
pub struct VotingAssignments {
    /// The slot this voting assignments is about.
    pub slot: Slot,
    /// The indices of validators that are attesting in this slot.
    pub attesting_validators: Vec<ValidatorIndex>,
    /// The pubkeys of attesting validators mapped to their attestation committee index.
    pub attesting_committees: HashMap<PublicKeyBytes, u64>,
    /// Sync committee validators mapped to their subnet IDs.
    /// A validator may participate in multiple subnets.
    pub sync_validators_by_subnet: HashMap<ValidatorIndex, HashSet<SyncSubnetId>>,
}

impl VotingAssignments {
    /// Returns a flat list of all sync validator indices.
    ///
    /// Derives this from the keys of `sync_validators_by_subnet`.
    pub fn sync_validators(&self) -> Vec<ValidatorIndex> {
        self.sync_validators_by_subnet.keys().copied().collect()
    }

    /// Counts expected signatures for selection proof collection.
    ///
    /// For each validator in the committee:
    /// - `+1` if the validator is attesting
    /// - `+N` if the validator is in sync committee (N = number of subnets)
    ///
    /// This counting pattern is used for aggregator committee pre-consensus where
    /// each sync validator produces one selection proof per subnet they participate in.
    pub fn selection_proof_count_for_committee<F>(&self, is_in_committee: F) -> usize
    where
        F: Fn(&ValidatorIndex) -> bool,
    {
        let mut count = 0;

        // Count attesting validators: +1 each
        for validator_idx in &self.attesting_validators {
            if is_in_committee(validator_idx) {
                count += 1;
            }
        }

        // Count sync validators: +N each (N = number of subnets)
        for (validator_idx, subnets) in &self.sync_validators_by_subnet {
            if is_in_committee(validator_idx) {
                count += subnets.len();
            }
        }

        count
    }

    /// Counts expected signatures for voting message collection.
    ///
    /// For each validator in the committee:
    /// - `+1` if the validator is attesting
    /// - `+1` if the validator is in sync committee (regardless of subnet count)
    ///
    /// This counting pattern is used for post-consensus attestation and sync committee
    /// voting messages where each validator produces one message regardless of
    /// how many subnets they participate in.
    pub fn voting_message_count_for_committee<F>(&self, is_in_committee: F) -> usize
    where
        F: Fn(&ValidatorIndex) -> bool,
    {
        let mut count = 0;

        // Count attesting validators: +1 each
        for validator_idx in &self.attesting_validators {
            if is_in_committee(validator_idx) {
                count += 1;
            }
        }

        // Count sync validators: +1 each (flat, regardless of subnet count)
        for validator_idx in self.sync_validators_by_subnet.keys() {
            if is_in_committee(validator_idx) {
                count += 1;
            }
        }

        count
    }
}

/// Aggregator-specific voting assignments, cached at 2/3 slot when selection proofs are known.
///
/// This struct is separate from `VotingAssignments` because:
/// - `VotingAssignments` is cached at slot start, before selection proofs are computed
/// - `AggregationAssignments` is cached at 2/3 slot, after Lighthouse fills in selection proofs
///
/// At 2/3 slot, `DutyAndProof.selection_proof.is_some()` indicates `is_aggregator = true`.
///
/// Also tracks multi-subnet sync aggregators. When a validator aggregates for multiple
/// sync subnets, `produce_signed_contribution_and_proof` is called multiple times (once
/// per subnet). The waiter ensures all contributions are collected before starting QBFT.
pub struct AggregationAssignments<E: EthSpec> {
    /// The slot this info is for
    pub slot: Slot,

    /// `Pubkey` -> `committee_index` for aggregating validators
    pub aggregator_committees: HashMap<PublicKeyBytes, u64>,

    /// Multi-subnet sync aggregators (validators aggregating > 1 subnet)
    multi_sync_aggregators: HashMap<PublicKeyBytes, ContributionWaiter<E>>,

    /// Pre-built consensus data per SSV committee (for Boole+)
    /// Maps `CommitteeId` -> `AggregatorCommitteeConsensusData`
    consensus_data_by_ssv_committee: HashMap<CommitteeId, Arc<AggregatorCommitteeConsensusData<E>>>,
}

impl<E: EthSpec> AggregationAssignments<E> {
    /// Get the pre-built consensus data for an SSV committee.
    /// Returns None if fork < Boole or no aggregators in committee.
    pub fn get_consensus_data(
        &self,
        ssv_committee_id: &CommitteeId,
    ) -> Option<Arc<AggregatorCommitteeConsensusData<E>>> {
        self.consensus_data_by_ssv_committee
            .get(ssv_committee_id)
            .cloned()
    }
}

pub struct ContributionWaiter<E: EthSpec> {
    data: RwLock<Vec<ContributionAndProofSigningData<E>>>,
    barrier: Barrier,
}

impl<E: EthSpec> ContributionWaiter<E> {
    fn new(count: usize) -> Self {
        Self {
            data: Default::default(),
            barrier: Barrier::new(count),
        }
    }

    async fn submit_and_wait(
        &self,
        data: ContributionAndProofSigningData<E>,
    ) -> Vec<ContributionAndProofSigningData<E>> {
        self.data.write().await.push(data);
        select! {
            _ = self.barrier.wait() => {}
            _ = sleep(Duration::from_secs(1)) => {
                warn!("Contribution waiter timed out");
            }
        }
        (*self.data.read().await).clone()
    }
}

#[derive(Clone)]
pub struct ContributionAndProofSigningData<E: EthSpec> {
    contribution: SyncCommitteeContribution<E>,
    selection_proof: SyncSelectionProof,
}

enum CollectionMode {
    SingleValidator,
    Committee {
        num_signatures_to_collect: usize,
        base_hash: Hash256,
    },
}

#[derive(Debug, Clone)]
pub enum SpecificError {
    Unsupported,
    SignatureCollectionFailed(CollectionError),
    ArithError(ArithError),
    QbftError(QbftError),
    Timeout,
    InvalidQbftData(DecodeError),
    TooManySyncSubnetsToSign,
    NoDataAgreed,
    Metadata,
    MissingIndex,
    SlotClock,
    NotSynced,
    InconsistentDatabase,
    /// Database inconsistency: validator references a cluster that doesn't exist
    ValidatorClusterMismatch {
        validator_pubkey: PublicKeyBytes,
        cluster_id: ClusterId,
    },
    KeyShareDecryptionFailed,
    DataTooLarge(String),
    ClusterLiquidated,
    /// Requested slot has already passed the current cached slot in `VotingAssignments`
    MetadataSlotPassed,
    /// Watch channel for `VotingAssignments` has been closed
    MetadataChannelClosed,
    /// Requested slot has already passed the current cached slot in `AggregationAssignments`
    AggregatorInfoSlotPassed,
    /// Watch channel for `AggregationAssignments` has been closed
    AggregatorInfoChannelClosed,
    /// `produce_selection_proof` called for validator not in
    /// `VotingAssignments.attesting_committees`
    ValidatorNotAttesting {
        validator_pubkey: PublicKeyBytes,
        slot: Slot,
    },
    /// `produce_sync_selection_proof` called for validator not in
    /// `VotingAssignments.sync_validators_by_subnet`
    ValidatorNotInSyncCommittee {
        validator_pubkey: PublicKeyBytes,
        slot: Slot,
    },
    /// Pre-built consensus data not found for this committee (Boole+)
    ConsensusDataNotFound,
    /// This committee's aggregate not found in consensus data (Boole+)
    AggregateNotInConsensus(u64),
    /// This subcommittee's contribution not found in consensus data (Boole+)
    ContributionNotInConsensus(u64),
    /// This validator not found in consensus data (Boole+)
    ValidatorNotInConsensus(ValidatorIndex),
}

impl From<CollectionError> for SpecificError {
    fn from(err: CollectionError) -> SpecificError {
        SpecificError::SignatureCollectionFailed(err)
    }
}

impl From<ArithError> for SpecificError {
    fn from(err: ArithError) -> SpecificError {
        SpecificError::ArithError(err)
    }
}

impl From<QbftError> for SpecificError {
    fn from(err: QbftError) -> SpecificError {
        SpecificError::QbftError(err)
    }
}

fn convert_slashing_result(value: Result<Safe, NotSafe>) -> Result<(), Error> {
    match value {
        Ok(Safe::Valid) => Ok(()),
        Ok(Safe::SameData) => Err(Error::SameData),
        Err(not_safe) => Err(Error::Slashable(not_safe)),
    }
}

pub type Error = ValidatorStoreError<SpecificError>;

impl<T: SlotClock, E: EthSpec> ValidatorStore for AnchorValidatorStore<T, E> {
    type Error = SpecificError;
    type E = E;

    fn validator_index(&self, pubkey: &PublicKeyBytes) -> Option<u64> {
        self.database
            .state()
            .metadata()
            .get_by(pubkey)
            .and_then(|v| v.index.map(|idx| *idx as u64))
    }

    fn voting_pubkeys<I, F>(&self, filter_func: F) -> I
    where
        I: FromIterator<PublicKeyBytes>,
        F: Fn(DoppelgangerStatus) -> Option<PublicKeyBytes>,
    {
        let state = self.database.state();

        // Treat all shares as `SigningEnabled`
        state
            .shares()
            .values()
            .filter_map(|v| filter_func(DoppelgangerStatus::SigningEnabled(v.validator_pubkey)))
            .filter(|public_key| {
                state
                    .clusters()
                    .get_by(public_key)
                    .is_some_and(|cluster| !cluster.liquidated)
            })
            .collect()
    }

    fn doppelganger_protection_allows_signing(&self, _validator_pubkey: PublicKeyBytes) -> bool {
        // we don't care about doppelgangers
        true
    }

    fn num_voting_validators(&self) -> usize {
        self.database.state().shares().length()
    }

    fn graffiti(&self, validator_pubkey: &PublicKeyBytes) -> Option<Graffiti> {
        self.database
            .state()
            .metadata()
            .get_by(validator_pubkey)
            .map(|metadata| metadata.graffiti)
    }

    fn get_fee_recipient(&self, validator_pubkey: &PublicKeyBytes) -> Option<Address> {
        let state = self.database.state();
        state.metadata().get_by(validator_pubkey).and_then(|v| {
            state
                .clusters()
                .get_by(&v.cluster_id)
                .map(|cluster| cluster.fee_recipient)
        })
    }

    fn determine_builder_boost_factor(&self, _validator_pubkey: &PublicKeyBytes) -> Option<u64> {
        if self.prefer_builder_proposals {
            return Some(u64::MAX);
        }

        self.builder_boost_factor
    }

    async fn randao_reveal(
        &self,
        validator_pubkey: PublicKeyBytes,
        signing_epoch: Epoch,
    ) -> Result<Signature, Error> {
        let future = async {
            let domain_hash = self.get_domain(signing_epoch, Domain::Randao);
            let signing_root = signing_epoch.signing_root(domain_hash);

            let (validator, cluster) = self.get_validator_and_cluster(validator_pubkey)?;

            self.collect_signature(
                PartialSignatureKind::RandaoPartialSig,
                Role::Proposer,
                CollectionMode::SingleValidator,
                &validator,
                &cluster,
                signing_root,
                self.slot_clock.now().ok_or(SpecificError::SlotClock)?,
            )
            .await
        };

        run_and_update_metrics(
            RANDAO_REVEAL_LOG_NAME,
            &metrics::SIGNED_RANDAO_REVEALS_TOTAL,
            future,
        )
        .await
    }

    fn set_validator_index(&self, validator_pubkey: &PublicKeyBytes, index: u64) {
        let Some(maybe_old_idx) = self
            .database
            .state()
            .metadata()
            .get_by(validator_pubkey)
            .map(|v| v.index)
        else {
            warn!(
                validator = validator_pubkey.as_hex_string(),
                "Trying to set index for unknown validator"
            );
            return;
        };

        let index = ValidatorIndex(index as usize);
        if let Some(old_idx) = maybe_old_idx {
            if old_idx != index {
                error!(
                    ?validator_pubkey,
                    db=?old_idx,
                    got=?index,
                    "Inconsistent validator index - database corrupt?"
                );
            }
        } else {
            let result = self
                .database
                .set_validator_indices(HashMap::from([(*validator_pubkey, index)]));
            if let Err(err) = result {
                error!(?err, "Failed to set validator index");
            }
        }
    }

    async fn sign_block(
        &self,
        validator_pubkey: PublicKeyBytes,
        block: UnsignedBlock<E>,
        current_slot: Slot,
    ) -> Result<SignedBlock<E>, Error> {
        let future = async {
            if !*self.is_synced.borrow() {
                return Err(Error::SpecificError(SpecificError::NotSynced));
            }
            let (validator, cluster) = self.get_validator_and_cluster(validator_pubkey)?;

            let (blinded_block, proofs_and_blobs, block_full) = match block {
                UnsignedBlock::Full(FullBlockContents::BlockContents(contents)) => (
                    contents.block.to_ref().into(),
                    Some((contents.kzg_proofs, contents.blobs)),
                    Some(contents.block),
                ),
                UnsignedBlock::Full(FullBlockContents::Block(block)) => {
                    (block.to_ref().into(), None, Some(block))
                }
                UnsignedBlock::Blinded(block) => (block, None, None),
            };

            let decided_block = self
                .decide_abstract_block(&validator, &cluster, &blinded_block)
                .await?;

            // Sign the decided block
            let signed_block = match decided_block {
                UnsignedBlock::Blinded(block) => {
                    self.sign_abstract_block(&validator, &cluster, block, current_slot)
                        .await
                }
                UnsignedBlock::Full(block) => {
                    self.sign_abstract_block(
                        &validator,
                        &cluster,
                        BeaconBlock::from(block),
                        current_slot,
                    )
                    .await
                }
            }?;

            match signed_block {
                SignedBlock::Blinded(signed_blinded_block) => {
                    // Check if the decided block matches our original proposal
                    if signed_blinded_block.signed_block_header().message
                        == blinded_block.block_header()
                    {
                        if let Some(full_block) = block_full {
                            let signed_full_block = SignedBeaconBlock::from_block(
                                full_block,
                                signed_blinded_block.signature().clone(),
                            );
                            Ok(SignedBlock::Full(PublishBlockRequest::new(
                                Arc::new(signed_full_block),
                                proofs_and_blobs,
                            )))
                        } else {
                            Ok(SignedBlock::Blinded(signed_blinded_block))
                        }
                    } else {
                        // Someone else's proposal won, return blinded
                        Ok(SignedBlock::Blinded(signed_blinded_block))
                    }
                }
                SignedBlock::Full(signed_block) => Ok(SignedBlock::Full(signed_block)),
            }
        };

        run_and_update_metrics(
            BLOCK_LOG_NAME,
            &validator_metrics::SIGNED_BLOCKS_TOTAL,
            future,
        )
        .await
    }

    async fn sign_validator_registration_data(
        &self,
        validator_registration_data: ValidatorRegistrationData,
    ) -> Result<SignedValidatorRegistrationData, Error> {
        let future = async {
            let domain_hash = self.spec.get_builder_domain();

            let (validator, cluster) =
                self.get_validator_and_cluster(validator_registration_data.pubkey)?;

            // Go-SSV always uses the start of the current epoch for the timestamp in
            // `ValidatorRegistrationData`, so we need to convert to that. However, it uses the duty
            // slot (which is passed in) for the signature message, so we need to pass that to
            // `collect_signature`.
            let duty_slot = self
                .slot_clock
                .slot_of(Duration::from_secs(validator_registration_data.timestamp))
                .ok_or(SpecificError::SlotClock)?;
            let epoch_start_slot = duty_slot
                .epoch(E::slots_per_epoch())
                .start_slot(E::slots_per_epoch());
            let duration = self
                .slot_clock
                .start_of(epoch_start_slot)
                .ok_or(SpecificError::SlotClock)?;
            let validator_registration_data = ValidatorRegistrationData {
                timestamp: duration.as_secs(),
                ..validator_registration_data
            };

            let signing_root = validator_registration_data.signing_root(domain_hash);

            let signature = self
                .collect_signature(
                    PartialSignatureKind::ValidatorRegistration,
                    Role::ValidatorRegistration,
                    CollectionMode::SingleValidator,
                    &validator,
                    &cluster,
                    signing_root,
                    duty_slot,
                )
                .await?;

            Ok(SignedValidatorRegistrationData {
                message: validator_registration_data,
                signature,
            })
        };

        run_and_update_metrics(
            VALIDATOR_REGISTRATION_LOG_NAME,
            &validator_metrics::SIGNED_VALIDATOR_REGISTRATIONS_TOTAL,
            future,
        )
        .await
    }

    async fn produce_signed_aggregate_and_proof(
        &self,
        validator_pubkey: PublicKeyBytes,
        aggregator_index: u64,
        aggregate: Attestation<E>,
        selection_proof: SelectionProof,
    ) -> Result<SignedAggregateAndProof<E>, Error> {
        let future = async {
            let slot = aggregate.data().slot;
            let signing_epoch = aggregate.data().target.epoch;
            let (validator, cluster) = self.get_validator_and_cluster(validator_pubkey)?;
            let committee_id = cluster.committee_id();

            if self.fork_schedule.active_fork(signing_epoch) >= Fork::Boole {
                self.produce_signed_aggregate_and_proof_boole(
                    validator_pubkey,
                    aggregator_index,
                    signing_epoch,
                    slot,
                    validator,
                    cluster,
                    committee_id,
                )
                .await
            } else {
                self.produce_signed_aggregate_and_proof_alan(
                    validator_pubkey,
                    aggregator_index,
                    aggregate,
                    selection_proof,
                    signing_epoch,
                    validator,
                    cluster,
                )
                .await
            }
        };

        run_and_update_metrics(
            AGGREGATE_LOG_NAME,
            &validator_metrics::SIGNED_AGGREGATES_TOTAL,
            future,
        )
        .await
    }

    async fn produce_selection_proof(
        &self,
        validator_pubkey: PublicKeyBytes,
        slot: Slot,
    ) -> Result<SelectionProof, Error> {
        let future = async {
            let epoch = slot.epoch(E::slots_per_epoch());
            let domain_hash = self.get_domain(epoch, Domain::SelectionProof);
            let signing_root = slot.signing_root(domain_hash);
            let (validator, cluster) = self.get_validator_and_cluster(validator_pubkey)?;

            // We do not want to spend too long on the selection proof. We will not produce an
            // aggregation anyway if the proof is not known at 2/3rds into the slot - so we abort
            // then.
            let delay = Duration::from_secs(self.spec.seconds_per_slot) * 2 / 3;

            let signature = if self.fork_schedule.active_fork(epoch) >= Fork::Boole {
                let committee_id = cluster.committee_id();
                let voting_assignments = self.get_voting_assignments(slot).await?;

                // Defensive check: validator should be in `VotingAssignments` since both this
                // function call and `VotingAssignments` are derived from `DutiesService`. If not,
                // there's an inconsistency (e.g., stale cache after poll timeout) and we
                // should not participate with a wrong `num_signatures_to_collect`.
                if !voting_assignments
                    .attesting_committees
                    .contains_key(&validator_pubkey)
                {
                    return Err(SpecificError::ValidatorNotAttesting {
                        validator_pubkey,
                        slot,
                    }
                    .into());
                }

                // Build a set of validator indices in this committee.
                // This handles divergent operator views, since we only count validators we have
                // shares for.
                let committee_validator_indices =
                    self.get_committee_validator_indices(&committee_id);

                // Calculate how many selection proofs to collect using the selection proof counting
                // method.
                let num_signatures_to_collect = voting_assignments
                    .selection_proof_count_for_committee(|idx| {
                        committee_validator_indices.contains(idx)
                    });

                // Compute deterministic `base_hash` for batching (same across all operators in a
                // committee)
                let batch_id = SelectionProofBatchId::new(slot, committee_id);
                let base_hash = batch_id.hash();

                trace!(
                    %slot,
                    validator_index = ?validator.index,
                    "Producing committee selection proof"
                );

                let collection_mode = CollectionMode::Committee {
                    num_signatures_to_collect,
                    base_hash,
                };

                self.timeout_within_slot(
                    slot,
                    delay,
                    self.collect_signature(
                        PartialSignatureKind::AggregatorCommitteePartialSig,
                        Role::AggregatorCommittee,
                        collection_mode,
                        &validator,
                        &cluster,
                        signing_root,
                        slot,
                    ),
                )
                .await?
            } else {
                // Single validator collection (original behavior)
                self.timeout_within_slot(
                    slot,
                    delay,
                    self.collect_signature(
                        PartialSignatureKind::SelectionProofPartialSig,
                        Role::Aggregator,
                        CollectionMode::SingleValidator,
                        &validator,
                        &cluster,
                        signing_root,
                        slot,
                    ),
                )
                .await?
            };

            Ok(signature.into())
        };

        run_and_update_metrics(
            SELECTION_PROOF_LOG_NAME,
            &validator_metrics::SIGNED_SELECTION_PROOFS_TOTAL,
            future,
        )
        .await
    }

    async fn produce_sync_selection_proof(
        &self,
        validator_pubkey: &PublicKeyBytes,
        slot: Slot,
        subnet_id: SyncSubnetId,
    ) -> Result<SyncSelectionProof, Error> {
        let future = async {
            let epoch = slot.epoch(E::slots_per_epoch());
            let domain_hash = self.get_domain(epoch, Domain::SyncCommitteeSelectionProof);
            let signing_root = SyncAggregatorSelectionData {
                slot,
                subcommittee_index: subnet_id.into(),
            }
            .signing_root(domain_hash);
            let (validator, cluster) = self.get_validator_and_cluster(*validator_pubkey)?;

            // We do not want to spend too long on the selection proof. We will not produce an
            // aggregation anyway if the proof is not known at 2/3rds into the slot - so we abort
            // then.
            let delay = Duration::from_secs(self.spec.seconds_per_slot) * 2 / 3;

            let signature = if self.fork_schedule.active_fork(epoch) >= Fork::Boole {
                // Boole fork: Committee-based batching (batches with attestation selection proofs)
                let committee_id = cluster.committee_id();
                let voting_assignments = self.get_voting_assignments(slot).await?;

                // Defensive check: validator should be in `VotingAssignments` since both this
                // function call and `VotingAssignments` are derived from `DutiesService`. If not,
                // there's an inconsistency (e.g., stale cache after poll timeout) and we
                // should not participate with a wrong `num_signatures_to_collect`.
                let validator_index = validator.index.ok_or(SpecificError::MissingIndex)?;
                if !voting_assignments
                    .sync_validators_by_subnet
                    .contains_key(&validator_index)
                {
                    return Err(SpecificError::ValidatorNotInSyncCommittee {
                        validator_pubkey: *validator_pubkey,
                        slot,
                    }
                    .into());
                }

                // Build a set of validator indices in this committee.
                // This handles divergent operator views, since we only count validators we have
                // shares for.
                let committee_validator_indices =
                    self.get_committee_validator_indices(&committee_id);

                // Calculate how many selection proofs to collect using the selection proof counting
                // method.
                let num_signatures_to_collect = voting_assignments
                    .selection_proof_count_for_committee(|idx| {
                        committee_validator_indices.contains(idx)
                    });

                // Compute deterministic `base_hash` for batching (SAME as attestation selection
                // proofs). This ensures all selection proofs (attestation + sync) batch together
                // into one P2P message per committee.
                let batch_id = SelectionProofBatchId::new(slot, committee_id);
                let base_hash = batch_id.hash();

                trace!(
                    %slot,
                    ?validator_index,
                    ?subnet_id,
                    "Producing committee sync selection proof"
                );

                let collection_mode = CollectionMode::Committee {
                    num_signatures_to_collect,
                    base_hash,
                };

                self.timeout_within_slot(
                    slot,
                    delay,
                    self.collect_signature(
                        PartialSignatureKind::AggregatorCommitteePartialSig,
                        Role::AggregatorCommittee,
                        collection_mode,
                        &validator,
                        &cluster,
                        signing_root,
                        slot,
                    ),
                )
                .await?
            } else {
                // Single-validator collection (original behavior)
                self.timeout_within_slot(
                    slot,
                    delay,
                    self.collect_signature(
                        PartialSignatureKind::ContributionProofs, // Original Alan-only enum
                        Role::SyncCommittee,
                        CollectionMode::SingleValidator,
                        &validator,
                        &cluster,
                        signing_root,
                        slot,
                    ),
                )
                .await?
            };

            Ok(signature.into())
        };

        run_and_update_metrics(
            SYNC_SELECTION_PROOF_LOG_NAME,
            &validator_metrics::SIGNED_SYNC_SELECTION_PROOFS_TOTAL,
            future,
        )
        .await
    }

    async fn produce_sync_committee_signature(
        &self,
        slot: Slot,
        _beacon_block_root: Hash256,
        validator_index: u64,
        validator_pubkey: &PublicKeyBytes,
    ) -> Result<SyncCommitteeMessage, Error> {
        let future = async {
            let epoch = slot.epoch(E::slots_per_epoch());
            let (validator, cluster) = self.get_validator_and_cluster(*validator_pubkey)?;
            let metadata = self.get_voting_context(slot).await?;

            let validator_attestation_committees =
                self.get_attesting_validators_in_committee(&metadata, cluster.committee_id());

            let timer =
                metrics::start_timer_vec(&metrics::CONSENSUS_TIMES, &[metrics::BEACON_VOTE]);
            let timeout_mode = TimeoutMode::SlotTime {
                instance_start_time: self.get_instant_in_slot(
                    slot,
                    Duration::from_secs(self.spec.seconds_per_slot) / 3,
                )?,
            };
            let completed = self
                .qbft_manager
                .decide_instance(
                    CommitteeInstanceId {
                        committee: cluster.committee_id(),
                        instance_height: slot.as_usize().into(),
                    },
                    metadata.beacon_vote.clone(),
                    self.create_beacon_vote_validator(slot, validator_attestation_committees),
                    timeout_mode,
                    &cluster,
                )
                .await
                .map_err(SpecificError::from)?;
            drop(timer);

            let data = match completed {
                Completed::TimedOut => return Err(Error::SpecificError(SpecificError::Timeout)),
                Completed::Success(data) => data,
            };

            // Calculate signature count for post-consensus committee collection
            let committee_validator_indices =
                self.get_committee_validator_indices(&cluster.committee_id());

            // Use `voting_message_count_for_committee` for post-consensus (flat counting)
            let num_signatures_to_collect = metadata
                .voting_assignments
                .voting_message_count_for_committee(|idx| {
                    committee_validator_indices.contains(idx)
                });

            let domain = self.get_domain(epoch, Domain::SyncCommittee);
            let signing_root = data.block_root.signing_root(domain);
            let signature = self
                .collect_signature(
                    PartialSignatureKind::PostConsensus,
                    Role::Committee,
                    CollectionMode::Committee {
                        num_signatures_to_collect,
                        base_hash: data.hash(),
                    },
                    &validator,
                    &cluster,
                    signing_root,
                    slot,
                )
                .await?;

            Ok(SyncCommitteeMessage {
                slot,
                beacon_block_root: data.block_root,
                validator_index,
                signature,
            })
        };

        run_and_update_metrics(
            SYNC_COMMITTEE_SIGNATURE_LOG_NAME,
            &validator_metrics::SIGNED_SYNC_COMMITTEE_MESSAGES_TOTAL,
            future,
        )
        .await
    }

    async fn produce_signed_contribution_and_proof(
        &self,
        aggregator_index: u64,
        aggregator_pubkey: PublicKeyBytes,
        contribution: SyncCommitteeContribution<E>,
        selection_proof: SyncSelectionProof,
    ) -> Result<SignedContributionAndProof<E>, Error> {
        let future = async {
            let slot = contribution.slot;
            let epoch = slot.epoch(E::slots_per_epoch());
            let (validator, cluster) = self.get_validator_and_cluster(aggregator_pubkey)?;
            let committee_id = cluster.committee_id();
            let subcommittee_index = contribution.subcommittee_index;

            if self.fork_schedule.active_fork(epoch) >= Fork::Boole {
                self.produce_signed_contribution_and_proof_boole(
                    aggregator_index,
                    aggregator_pubkey,
                    subcommittee_index,
                    slot,
                    epoch,
                    validator,
                    cluster,
                    committee_id,
                )
                .await
            } else {
                self.produce_signed_contribution_and_proof_alan(
                    aggregator_index,
                    aggregator_pubkey,
                    contribution,
                    selection_proof,
                    slot,
                    epoch,
                    subcommittee_index,
                    validator,
                    cluster,
                )
                .await
            }
        };
        run_and_update_metrics(
            SYNC_COMMITTEE_CONTRIBUTION_LOG_NAME,
            &validator_metrics::SIGNED_SYNC_COMMITTEE_CONTRIBUTIONS_TOTAL,
            future,
        )
        .await
    }

    // stolen from lighthouse
    /// Prune the slashing protection database so that it remains performant.
    ///
    /// This function will only do actual pruning periodically, so it should usually be
    /// cheap to call. The `first_run` flag can be used to print a more verbose message when pruning
    /// runs.
    fn prune_slashing_protection_db(&self, current_epoch: Epoch, first_run: bool) {
        // Attempt to prune every SLASHING_PROTECTION_HISTORY_EPOCHs, with a tolerance for
        // missing the epoch that aligns exactly.
        let mut last_prune = self.slashing_protection_last_prune.lock();
        if current_epoch / SLASHING_PROTECTION_HISTORY_EPOCHS
            <= *last_prune / SLASHING_PROTECTION_HISTORY_EPOCHS
        {
            return;
        }

        if first_run {
            info!(
                "epoch" = %current_epoch,
                "msg" = "pruning may take several minutes the first time it runs",
                "Pruning slashing protection DB",
            );
        } else {
            info!(
                "epoch" = %current_epoch,
                "Pruning slashing protection DB",
            );
        }

        let _timer =
            validator_metrics::start_timer(&validator_metrics::SLASHING_PROTECTION_PRUNE_TIMES);

        let new_min_target_epoch = current_epoch.saturating_sub(SLASHING_PROTECTION_HISTORY_EPOCHS);
        let new_min_slot = new_min_target_epoch.start_slot(E::slots_per_epoch());

        let all_pubkeys: Vec<_> = self.voting_pubkeys(DoppelgangerStatus::ignored);

        if let Err(e) = self
            .slashing_protection
            .prune_all_signed_attestations(all_pubkeys.iter(), new_min_target_epoch)
        {
            error!(
                "error" = ?e,
                "Error during pruning of signed attestations",
            );
            return;
        }

        if let Err(e) = self
            .slashing_protection
            .prune_all_signed_blocks(all_pubkeys.iter(), new_min_slot)
        {
            error!(
                "error" = ?e,
                "Error during pruning of signed blocks",
            );
            return;
        }

        *last_prune = current_epoch;

        info!("Completed pruning of slashing protection DB");
    }

    fn proposal_data(&self, pubkey: &PublicKeyBytes) -> Option<ProposalData> {
        let state = self.database.state();
        let validator = state.metadata().get_by(pubkey)?;

        let validator_index = validator.index.map(|idx| *idx as u64);
        let cluster = state.clusters().get_by(&validator.cluster_id);
        let fee_recipient = cluster.map(|c| c.fee_recipient);

        Some(ProposalData {
            validator_index,
            fee_recipient,
            gas_limit: self.gas_limit,
            builder_proposals: true,
        })
    }

    async fn sign_attestations(
        self: &Arc<Self>,
        mut attestations: Vec<(u64, PublicKeyBytes, usize, Attestation<E>)>,
    ) -> Result<Vec<(u64, Attestation<E>)>, Error> {
        if !*self.is_synced.borrow() {
            return Err(Error::SpecificError(SpecificError::NotSynced));
        }

        let signing_futures = attestations.iter_mut().map(
            |(_, pubkey, validator_committee_index, attestation)| async move {
                self.sign_attestation(
                    *pubkey,
                    *validator_committee_index,
                    attestation,
                    attestation.data().target.epoch,
                )
                .await
            },
        );

        let results = join_all(signing_futures).await;

        let mut signed_attestations = Vec::with_capacity(results.len());
        for (result, (validator_index, pubkey, _, attestation)) in
            results.into_iter().zip(attestations.into_iter())
        {
            match result {
                Ok(()) => {
                    signed_attestations.push((validator_index, attestation, pubkey));
                }
                Err(ValidatorStoreError::UnknownPubkey(pubkey)) => {
                    warn!(
                        info = "a validator may have recently been removed from this VC",
                        ?pubkey,
                        "Missing pubkey for attestation"
                    );
                }
                Err(e) => {
                    error!(
                        error = ?e,
                        "Failed to sign attestation"
                    );
                }
            }
        }

        if signed_attestations.is_empty() {
            return Ok(vec![]);
        }

        // Check slashing protection and insert into database. Use a dedicated blocking thread
        // to avoid clogging the async executor with blocking database I/O.
        let validator_store = self.clone();
        self.task_executor
            .spawn_blocking_handle(
                move || validator_store.slashing_protection_attestations(signed_attestations),
                "slashing_protect_attestations",
            )
            .ok_or(Error::ExecutorError)?
            .await
            .map_err(|_| Error::ExecutorError)?
    }
}

trait SignableBlock<E: EthSpec>: Debug + Encode {
    type Payload: AbstractExecPayload<E>;

    fn as_block(&self) -> BeaconBlockRef<'_, E, Self::Payload>;
    fn to_signed_block(self, signature: Signature) -> SignedBlock<E>;
}

impl<E: EthSpec> SignableBlock<E> for BlockContents<E> {
    type Payload = FullPayload<E>;

    fn as_block(&self) -> BeaconBlockRef<'_, E, Self::Payload> {
        self.block.to_ref()
    }

    fn to_signed_block(self, signature: Signature) -> SignedBlock<E> {
        SignedBlock::Full(PublishBlockRequest::new(
            Arc::new(SignedBeaconBlock::from_block(self.block, signature)),
            Some((self.kzg_proofs, self.blobs)),
        ))
    }
}

impl<E: EthSpec> SignableBlock<E> for BeaconBlock<E, FullPayload<E>> {
    type Payload = FullPayload<E>;

    fn as_block(&self) -> BeaconBlockRef<'_, E, Self::Payload> {
        self.to_ref()
    }

    fn to_signed_block(self, signature: Signature) -> SignedBlock<E> {
        SignedBlock::Full(PublishBlockRequest::new(
            Arc::new(SignedBeaconBlock::from_block(self, signature)),
            None,
        ))
    }
}

impl<E: EthSpec> SignableBlock<E> for BeaconBlock<E, BlindedPayload<E>> {
    type Payload = BlindedPayload<E>;

    fn as_block(&self) -> BeaconBlockRef<'_, E, Self::Payload> {
        self.to_ref()
    }

    fn to_signed_block(self, signature: Signature) -> SignedBlock<E> {
        SignedBlock::Blinded(Arc::new(SignedBlindedBeaconBlock::from_block(
            self, signature,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a test `VotingAssignments` with the given parameters.
    fn create_test_voting_assignments(
        attesting_validators: Vec<usize>,
        sync_validators_by_subnet: Vec<(usize, Vec<u64>)>,
    ) -> VotingAssignments {
        VotingAssignments {
            slot: Slot::new(100),
            attesting_validators: attesting_validators
                .into_iter()
                .map(ValidatorIndex)
                .collect(),
            attesting_committees: HashMap::new(),
            sync_validators_by_subnet: sync_validators_by_subnet
                .into_iter()
                .map(|(idx, subnets)| {
                    (
                        ValidatorIndex(idx),
                        subnets.into_iter().map(SyncSubnetId::new).collect(),
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn test_selection_proof_count_with_multi_subnet_validators() {
        // Create voting assignments with:
        // - Validators 1, 2 attesting
        // - Validator 3 in 1 subnet (contributes 1)
        // - Validator 4 in 3 subnets (contributes 3)
        // - Validator 5 in 2 subnets (contributes 2)
        let voting_assignments = create_test_voting_assignments(
            vec![1, 2],
            vec![(3, vec![0]), (4, vec![0, 1, 2]), (5, vec![0, 1])],
        );

        // All validators in committee
        let all_in_committee = |_: &ValidatorIndex| true;
        let count = voting_assignments.selection_proof_count_for_committee(all_in_committee);
        // 2 attesting + 1 + 3 + 2 sync = 8
        assert_eq!(count, 8);
    }

    #[test]
    fn test_selection_proof_count_with_filter() {
        let voting_assignments = create_test_voting_assignments(
            vec![1, 2, 3],
            vec![
                (4, vec![0, 1]),    // 2 subnets
                (5, vec![0, 1, 2]), // 3 subnets
            ],
        );

        // Only validators 1, 2, 4 are in the committee
        let in_committee = |idx: &ValidatorIndex| matches!(idx.0, 1 | 2 | 4);
        let count = voting_assignments.selection_proof_count_for_committee(in_committee);
        // 2 attesting (1, 2) + 2 sync subnets (validator 4) = 4
        assert_eq!(count, 4);
    }

    #[test]
    fn test_committee_message_count_with_multi_subnet_validators() {
        // Same setup as selection proof test, but counting should be flat
        let voting_assignments = create_test_voting_assignments(
            vec![1, 2],
            vec![
                (3, vec![0]),
                (4, vec![0, 1, 2]), // 3 subnets but counts as 1
                (5, vec![0, 1]),    // 2 subnets but counts as 1
            ],
        );

        let all_in_committee = |_: &ValidatorIndex| true;
        let count = voting_assignments.voting_message_count_for_committee(all_in_committee);
        // 2 attesting + 3 sync validators (flat) = 5
        assert_eq!(count, 5);
    }

    #[test]
    fn test_committee_message_count_with_filter() {
        let voting_assignments = create_test_voting_assignments(
            vec![1, 2, 3],
            vec![
                (4, vec![0, 1]),    // 2 subnets
                (5, vec![0, 1, 2]), // 3 subnets
            ],
        );

        // Only validators 1, 2, 4 are in the committee
        let in_committee = |idx: &ValidatorIndex| matches!(idx.0, 1 | 2 | 4);
        let count = voting_assignments.voting_message_count_for_committee(in_committee);
        // 2 attesting (1, 2) + 1 sync (validator 4) = 3
        assert_eq!(count, 3);
    }

    #[test]
    fn test_counting_difference_between_methods() {
        // Demonstrate the key difference between the two counting methods
        let voting_assignments = create_test_voting_assignments(
            vec![1], // 1 attesting validator
            vec![
                (2, vec![0, 1, 2, 3]), // Validator in 4 subnets
            ],
        );

        let all_in_committee = |_: &ValidatorIndex| true;

        // Selection proof: 1 + 4 = 5
        let selection_count =
            voting_assignments.selection_proof_count_for_committee(all_in_committee);
        assert_eq!(selection_count, 5);

        // Voting message: 1 + 1 = 2
        let message_count = voting_assignments.voting_message_count_for_committee(all_in_committee);
        assert_eq!(message_count, 2);

        // The difference highlights the counting patterns:
        // - Selection proofs need one proof per subnet per validator
        // - Voting messages need one message per validator regardless of subnets
    }

    #[test]
    fn test_overlapping_attesting_and_sync_validators() {
        // Validator can be both attesting and in sync committee
        let mut voting_assignments = create_test_voting_assignments(
            vec![1, 2], // Validators 1 and 2 attesting
            vec![
                (1, vec![0]),    // Validator 1 also in sync (1 subnet)
                (2, vec![0, 1]), // Validator 2 also in sync (2 subnets)
            ],
        );
        voting_assignments.attesting_validators = vec![ValidatorIndex(1), ValidatorIndex(2)];

        let all_in_committee = |_: &ValidatorIndex| true;

        // Selection proof: 2 attesting + (1 + 2) sync = 5
        let selection_count =
            voting_assignments.selection_proof_count_for_committee(all_in_committee);
        assert_eq!(selection_count, 5);

        // Voting message: 2 attesting + 2 sync = 4
        let message_count = voting_assignments.voting_message_count_for_committee(all_in_committee);
        assert_eq!(message_count, 4);
    }

    #[tokio::test]
    async fn test_validator_voting_assignments_watch_channel_waits_for_update() {
        // Test the watch channel behavior directly without creating a full AnchorValidatorStore
        let (tx, mut rx) = watch::channel::<Option<Arc<VotingAssignments>>>(None);

        // Start a task to wait for slot 5
        let wait_task = tokio::spawn(async move {
            loop {
                let current = rx.borrow().clone();
                if let Some(voting_assignments) = current
                    && voting_assignments.slot == Slot::new(5)
                {
                    return Ok::<_, ()>(voting_assignments);
                }
                // Wait for update
                if rx.changed().await.is_err() {
                    return Err(());
                }
            }
        });

        // Give the task time to start waiting
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Update with voting assignments for slot 5
        let voting_assignments = VotingAssignments {
            slot: Slot::new(5),
            attesting_validators: vec![ValidatorIndex(1)],
            attesting_committees: HashMap::new(),
            sync_validators_by_subnet: HashMap::new(),
        };
        tx.send_replace(Some(Arc::new(voting_assignments)));

        // The wait task should now complete successfully
        let result = wait_task.await.unwrap();
        assert!(result.is_ok());
        let received = result.unwrap();
        assert_eq!(received.slot, Slot::new(5));
        assert_eq!(received.attesting_validators, vec![ValidatorIndex(1)]);
    }

    #[tokio::test]
    async fn test_validator_voting_assignments_errors_if_slot_passed() {
        // Test the watch channel behavior directly
        let (tx, rx) = watch::channel::<Option<Arc<VotingAssignments>>>(None);

        // Update with voting assignments for slot 10 (newer than what we'll request)
        let voting_assignments = VotingAssignments {
            slot: Slot::new(10),
            attesting_validators: vec![],
            attesting_committees: HashMap::new(),
            sync_validators_by_subnet: HashMap::new(),
        };
        tx.send_replace(Some(Arc::new(voting_assignments)));

        // Simulate requesting slot 5 (older than cached slot 10)
        let current = rx.borrow().clone();
        if let Some(voting_assignments) = current {
            if voting_assignments.slot > Slot::new(5) {
                // This simulates the error condition we'd return
                assert_eq!(voting_assignments.slot, Slot::new(10));
            } else {
                panic!("Should have newer slot cached");
            }
        } else {
            panic!("Should have voting assignments cached");
        }
    }
}

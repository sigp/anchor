pub mod metadata_service;
mod metrics;

use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    future::Future,
    str::from_utf8,
    sync::{Arc, LazyLock},
    time::Duration,
};

use dashmap::DashMap;
use database::{NetworkState, NonUniqueIndex, UniqueIndex};
use eth2::types::{BlockContents, FullBlockContents, PublishBlockRequest};
use openssl::{
    pkey::Private,
    rsa::{Padding, Rsa},
};
use parking_lot::Mutex;
use qbft::Completed;
use qbft_manager::{
    CommitteeInstanceId, QbftError, QbftManager, ValidatorDutyKind, ValidatorInstanceId,
};
use safe_arith::{ArithError, SafeArith};
use signature_collector::{
    CollectionError, SignatureCollectorManager, SignatureMetadata, SignatureRequester,
    ValidatorSigningData,
};
use slashing_protection::{NotSafe, Safe, SlashingDatabase};
use slot_clock::SlotClock;
use ssv_types::{
    Cluster, CommitteeId, ValidatorIndex, ValidatorMetadata,
    consensus::{
        BEACON_ROLE_AGGREGATOR, BEACON_ROLE_PROPOSER, BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION,
        BeaconVote, Contribution, ContributionWrapper, Contributions, QbftData,
        ValidatorConsensusData, ValidatorDuty,
    },
    msgid::Role,
    partial_sig::PartialSignatureKind,
};
use ssz::{Decode, DecodeError, Encode};
use task_executor::TaskExecutor;
use tokio::{
    select,
    sync::{Barrier, RwLock, watch},
    time::{Instant, sleep},
};
use tracing::{debug, error, info, warn};
use types::{
    AbstractExecPayload, Address, AggregateAndProof, AggregateAndProofBase,
    AggregateAndProofElectra, BeaconBlockRef, BlindedBeaconBlock, BlindedPayload, ChainSpec,
    ContributionAndProof, Domain, EthSpec, ForkName, FullPayload, Hash256, PublicKeyBytes,
    SecretKey, Signature, SignedBeaconBlock, SignedBlindedBeaconBlock, SignedRoot,
    SignedVoluntaryExit, SyncAggregatorSelectionData, VoluntaryExit,
    attestation::Attestation,
    beacon_block::BeaconBlock,
    graffiti::Graffiti,
    selection_proof::SelectionProof,
    signed_aggregate_and_proof::SignedAggregateAndProof,
    signed_contribution_and_proof::SignedContributionAndProof,
    slot_data::SlotData,
    slot_epoch::{Epoch, Slot},
    sync_committee_contribution::SyncCommitteeContribution,
    sync_committee_message::SyncCommitteeMessage,
    sync_selection_proof::SyncSelectionProof,
    sync_subnet_id::SyncSubnetId,
    validator_registration_data::{SignedValidatorRegistrationData, ValidatorRegistrationData},
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

const RANDAO_REVEAL_LOG_NAME: &str = "RANDAO reveal";
const BLOCK_LOG_NAME: &str = "block";
const ATTESTATION_LOG_NAME: &str = "attestation";
const VALIDATOR_REGISTRATION_LOG_NAME: &str = "validator registration";
const AGGREGATE_LOG_NAME: &str = "aggregate";
const SELECTION_PROOF_LOG_NAME: &str = "selection proof";
const SYNC_SELECTION_PROOF_LOG_NAME: &str = "sync selection proof";
const SYNC_COMMITTEE_SIGNATURE_LOG_NAME: &str = "sync committee signature";
const SYNC_COMMITTEE_CONTRIBUTION_LOG_NAME: &str = "sync committee contribution";

#[derive(Clone)]
struct InitializedValidator {
    cluster: Cluster,
    metadata: ValidatorMetadata,
    decrypted_key_share: Option<SecretKey>,
}

pub struct AnchorValidatorStore<T: SlotClock + 'static, E: EthSpec> {
    validators: DashMap<PublicKeyBytes, InitializedValidator>,
    validators_per_committee: DashMap<CommitteeId, HashSet<ValidatorIndex>>,
    signature_collector: Arc<SignatureCollectorManager>,
    qbft_manager: Arc<QbftManager>,
    slashing_protection: SlashingDatabase,
    slashing_protection_last_prune: Mutex<Epoch>,
    disable_slashing_protection: bool,
    slot_clock: T,
    spec: Arc<ChainSpec>,
    genesis_validators_root: Hash256,
    private_key: Option<Rsa<Private>>,
    slot_metadata: watch::Sender<Option<Arc<SlotMetadata<E>>>>,
    gas_limit: u64,
    // MEV configuration is applied at the operator level and applies to all validators this
    // operator controls
    builder_proposals: bool,
    builder_boost_factor: Option<u64>,
    prefer_builder_proposals: bool,
}

impl<T: SlotClock, E: EthSpec> AnchorValidatorStore<T, E> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        database_state: watch::Receiver<NetworkState>,
        signature_collector: Arc<SignatureCollectorManager>,
        qbft_manager: Arc<QbftManager>,
        slashing_protection: SlashingDatabase,
        disable_slashing_protection: bool,
        slot_clock: T,
        spec: Arc<ChainSpec>,
        genesis_validators_root: Hash256,
        private_key: Option<Rsa<Private>>,
        task_executor: TaskExecutor,
        gas_limit: u64,
        builder_proposals: bool,
        builder_boost_factor: Option<u64>,
        prefer_builder_proposals: bool,
    ) -> Arc<AnchorValidatorStore<T, E>> {
        let ret = Arc::new(Self {
            validators: DashMap::new(),
            validators_per_committee: DashMap::new(),
            signature_collector,
            qbft_manager,
            slashing_protection,
            slashing_protection_last_prune: Mutex::new(Epoch::new(0)),
            disable_slashing_protection,
            slot_clock,
            spec,
            genesis_validators_root,
            private_key,
            slot_metadata: watch::channel(None).0,
            gas_limit,
            builder_proposals,
            builder_boost_factor,
            prefer_builder_proposals,
        });

        task_executor.spawn(
            Arc::clone(&ret).updater(database_state),
            "validator_store_updater",
        );

        ret
    }

    async fn updater(self: Arc<Self>, mut database_state: watch::Receiver<NetworkState>) {
        while database_state.changed().await.is_ok() {
            self.load_validators(&database_state.borrow());
        }
    }

    fn load_validators(&self, state: &NetworkState) {
        let mut unseen_validators = self
            .validators
            .iter()
            .map(|v| *v.key())
            .collect::<HashSet<_>>();
        let db_clusters = state.get_own_clusters().iter().collect::<Vec<_>>();

        for (cluster, validator) in db_clusters
            .into_iter()
            .filter_map(|id| state.clusters().get_by(id))
            .filter(|cluster| !cluster.liquidated)
            .flat_map(|cluster| {
                state
                    .metadata()
                    .get_all_by(&cluster.cluster_id)
                    .map(move |metadata| (cluster, metadata))
            })
        {
            if unseen_validators.remove(&validator.public_key) {
                // Validator was present: check if the cluster has changed
                if let Some(mut entry) = self.validators.get_mut(&validator.public_key) {
                    let current_cluster = &mut entry.value_mut().cluster;
                    if current_cluster.cluster_id != cluster.cluster_id {
                        // Update the validator with the new cluster
                        *current_cluster = cluster.clone();
                    }
                }
            } else {
                // value was not present: add to store
                if let Ok(secret_key) =
                    self.get_share_from_state(state, validator, validator.public_key)
                {
                    let result = self.add_validator(
                        validator.public_key,
                        cluster,
                        validator.clone(),
                        secret_key,
                    );
                    if let Err(err) = result {
                        error!(?err, "Unable to initialize validator");
                    }
                }
            }
        }

        for validator in unseen_validators {
            self.remove_validator(&validator);
            info!(%validator, "Validator disabled");
        }

        let count = self.validators.len() as i64;
        validator_metrics::set_gauge(&validator_metrics::ENABLED_VALIDATORS_COUNT, count);
        validator_metrics::set_gauge(&validator_metrics::TOTAL_VALIDATORS_COUNT, count);
    }

    fn get_share_from_state(
        &self,
        state: &NetworkState,
        validator: &ValidatorMetadata,
        pubkey_bytes: PublicKeyBytes,
    ) -> Result<Option<SecretKey>, ()> {
        // If we have no private key, we are running in impostor mode - so we can not decrypt the
        // share. Return `None` to let the signature collector mock the signing.
        let Some(private_key) = &self.private_key else {
            return Ok(None);
        };

        let share = state
            .shares()
            .get_by(&validator.public_key)
            .ok_or_else(|| warn!(validator = %pubkey_bytes, "Key share not found"))?;

        // the buffer size must be larger than or equal the modulus size
        let mut key_hex = [0; 2048 / 8];
        let length = private_key
            .private_decrypt(&share.encrypted_private_key, &mut key_hex, Padding::PKCS1)
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
            .map(Some)
            .map_err(|err| error!(?err, validator = %pubkey_bytes, "Invalid secret key decrypted"))
    }

    fn add_validator(
        &self,
        pubkey_bytes: PublicKeyBytes,
        cluster: &Cluster,
        validator_metadata: ValidatorMetadata,
        decrypted_key_share: Option<SecretKey>,
    ) -> Result<(), Error> {
        if let Some(index) = validator_metadata.index {
            self.validators_per_committee
                .entry(cluster.committee_id())
                .or_default()
                .insert(index);
        }

        self.validators.insert(
            pubkey_bytes,
            InitializedValidator {
                cluster: cluster.clone(),
                metadata: validator_metadata,
                decrypted_key_share,
            },
        );

        self.slashing_protection
            .register_validator(pubkey_bytes)
            .map_err(Error::Slashable)?;
        info!(validator = %pubkey_bytes, "Validator enabled");
        Ok(())
    }

    fn remove_validator(&self, pubkey_bytes: &PublicKeyBytes) {
        let Some((_, validator)) = self.validators.remove(pubkey_bytes) else {
            return;
        };
        if let Some(idx) = validator.metadata.index {
            for mut committee in self.validators_per_committee.iter_mut() {
                committee.remove(&idx);
            }
        }
    }

    fn validator(&self, validator_pubkey: PublicKeyBytes) -> Result<InitializedValidator, Error> {
        self.validators
            .get(&validator_pubkey)
            .map(|c| c.value().clone())
            .ok_or(Error::UnknownPubkey(validator_pubkey))
    }

    fn get_domain(&self, epoch: Epoch, domain: Domain) -> Hash256 {
        self.spec.get_domain(
            epoch,
            domain,
            &self.spec.fork_at_epoch(epoch),
            self.genesis_validators_root,
        )
    }

    async fn collect_signature(
        &self,
        signature_kind: PartialSignatureKind,
        role: Role,
        base_hash: Option<Hash256>,
        validator: InitializedValidator,
        signing_root: Hash256,
        slot: Slot,
    ) -> Result<Signature, Error> {
        let committee_id = validator.cluster.committee_id();
        let metadata = SignatureMetadata {
            kind: signature_kind,
            role,
            threshold: validator
                .cluster
                .get_f()
                .safe_mul(2)
                .and_then(|x| x.safe_add(1))
                .map_err(SpecificError::from)?,
            slot,
            committee_id,
        };

        let requester = if let Some(base_hash) = base_hash {
            let metadata = self.get_slot_metadata(slot).await?;
            SignatureRequester::Committee {
                num_signatures_to_collect: self
                    .validators_per_committee
                    .get(&committee_id)
                    .map(|indices| {
                        indices
                            .iter()
                            .map(|idx| {
                                let mut duties = 0;
                                if metadata.attesting_validators.contains(idx) {
                                    duties += 1;
                                }
                                if metadata.sync_validators.contains(idx) {
                                    duties += 1;
                                }
                                duties
                            })
                            .sum()
                    })
                    .unwrap_or_default(),
                base_hash,
            }
        } else {
            SignatureRequester::SingleValidator {
                pubkey: validator.metadata.public_key,
            }
        };

        let signing_data = ValidatorSigningData {
            root: signing_root,
            index: validator
                .metadata
                .index
                .ok_or(SpecificError::MissingIndex)?,
            share: validator.decrypted_key_share.clone(),
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
        validator_pubkey: PublicKeyBytes,
        signable_block: impl SignableBlock<E>,
    ) -> Result<UnsignedBlock<E>, Error> {
        let validator = self.validator(validator_pubkey)?;

        let block = signable_block.as_block();
        let slot = block.slot();

        // first, we have to get to consensus
        let timer = metrics::start_timer_vec(&metrics::CONSENSUS_TIMES, &[metrics::BLOCK]);
        let start_time = self.get_instant_in_slot(slot, Duration::ZERO)?;

        // Define the validator instance identity for QBFT consensus
        let instance_id = ValidatorInstanceId {
            validator: validator_pubkey,
            duty: ValidatorDutyKind::Proposal,
            instance_height: slot.as_usize().into(),
        };

        // Get the validator index, ensuring it exists
        let validator_index = validator
            .metadata
            .index
            .ok_or(SpecificError::MissingIndex)?;

        // Determine the appropriate version based on block type
        let block_version = block.fork_name_unchecked().into();

        // Create the validator duty information
        let validator_duty = ValidatorDuty {
            r#type: BEACON_ROLE_PROPOSER,
            pub_key: validator_pubkey,
            slot,
            validator_index,
            committee_index: 0,
            committee_length: 0,
            committees_at_slot: 0,
            validator_committee_index: 0,
            validator_sync_committee_indices: Default::default(),
        };

        // Package the consensus data
        let consensus_data = ValidatorConsensusData {
            duty: validator_duty,
            version: block_version,
            data_ssz: signable_block.as_ssz_bytes(),
        };

        // Initiate QBFT consensus for this block proposal
        let completed = self
            .qbft_manager
            .decide_instance(instance_id, consensus_data, start_time, &validator.cluster)
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
        validator_pubkey: PublicKeyBytes,
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
                &validator_pubkey,
                &header,
                domain_hash,
            ))?;
        }

        let signing_root = block.signing_root(domain_hash);
        let signature = self
            .collect_signature(
                PartialSignatureKind::PostConsensus,
                Role::Proposer,
                None,
                self.validator(validator_pubkey)?,
                signing_root,
                header.slot,
            )
            .await?;
        Ok(signable_block.to_signed_block(signature))
    }

    async fn get_slot_metadata(&self, slot: Slot) -> Result<Arc<SlotMetadata<E>>, Error> {
        let Some(metadata) = self
            .slot_metadata
            .subscribe()
            .wait_for(|m| m.as_ref().is_some_and(|metadata| metadata.slot >= slot))
            .await
            .ok()
            .and_then(|metadata| metadata.clone())
        else {
            error!("Unexpected error while waiting for metadata");
            return Err(Error::SpecificError(SpecificError::Metadata));
        };

        if metadata.slot == slot {
            Ok(metadata.clone())
        } else {
            error!("Got newer metadata - performance issues?");
            Err(Error::SpecificError(SpecificError::Metadata))
        }
    }

    fn update_slot_metadata(&self, metadata: SlotMetadata<E>) {
        self.slot_metadata.send_replace(Some(Arc::new(metadata)));
    }

    /// Return [`SpecificError::Timeout`] if the given future does not complete at `delay` into the
    /// given slot.
    ///
    /// In the unlikely case the `slot_clock` errors, we time out after `delay`;
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

        let signature = self
            .collect_signature(
                PartialSignatureKind::VoluntaryExit,
                Role::VoluntaryExit,
                None,
                self.validator(validator_pubkey)?,
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

struct SlotMetadata<E: EthSpec> {
    /// The slot this metadata is about.
    slot: Slot,
    /// The BeaconVote we will use as initial QBFT data.
    beacon_vote: BeaconVote,
    /// All our validators that are attesting in this slot.
    attesting_validators: Vec<ValidatorIndex>,
    /// All our validators that are in the sync committee for this slot.
    sync_validators: Vec<ValidatorIndex>,
    /// All validators that are aggregator for this slot multiple times, and thus require special
    /// synchronization.
    multi_sync_aggregators: HashMap<PublicKeyBytes, ContributionWaiter<E>>,
}

struct ContributionWaiter<E: EthSpec> {
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
        self.validator(*pubkey)
            .ok()
            .and_then(|v| v.metadata.index.map(|idx| *idx as u64))
    }

    fn voting_pubkeys<I, F>(&self, filter_func: F) -> I
    where
        I: FromIterator<PublicKeyBytes>,
        F: Fn(DoppelgangerStatus) -> Option<PublicKeyBytes>,
    {
        // Treat all validators as `SigningEnabled`
        self.validators
            .iter()
            .filter_map(|v| filter_func(DoppelgangerStatus::SigningEnabled(*v.key())))
            .collect()
    }

    fn doppelganger_protection_allows_signing(&self, _validator_pubkey: PublicKeyBytes) -> bool {
        // we don't care about doppelgangers
        true
    }

    fn num_voting_validators(&self) -> usize {
        self.validators.len()
    }

    fn graffiti(&self, validator_pubkey: &PublicKeyBytes) -> Option<Graffiti> {
        self.validator(*validator_pubkey)
            .ok()
            .map(|v| v.metadata.graffiti)
    }

    fn get_fee_recipient(&self, validator_pubkey: &PublicKeyBytes) -> Option<Address> {
        self.validator(*validator_pubkey)
            .ok()
            .map(|v| v.cluster.fee_recipient)
    }

    fn determine_builder_boost_factor(&self, _validator_pubkey: &PublicKeyBytes) -> Option<u64> {
        if self.prefer_builder_proposals {
            return Some(u64::MAX);
        }

        self.builder_boost_factor.or_else(|| {
            if !self.builder_proposals {
                return Some(0);
            }
            None
        })
    }

    async fn randao_reveal(
        &self,
        validator_pubkey: PublicKeyBytes,
        signing_epoch: Epoch,
    ) -> Result<Signature, Error> {
        let domain_hash = self.get_domain(signing_epoch, Domain::Randao);
        let signing_root = signing_epoch.signing_root(domain_hash);

        run_and_update_metrics(
            RANDAO_REVEAL_LOG_NAME,
            &metrics::SIGNED_RANDAO_REVEALS_TOTAL,
            self.collect_signature(
                PartialSignatureKind::RandaoPartialSig,
                Role::Proposer,
                None,
                self.validator(validator_pubkey)?,
                signing_root,
                self.slot_clock.now().ok_or(SpecificError::SlotClock)?,
            ),
        )
        .await
    }

    fn set_validator_index(&self, validator_pubkey: &PublicKeyBytes, index: u64) {
        match self.validators.get_mut(validator_pubkey) {
            None => warn!(
                validator = validator_pubkey.as_hex_string(),
                "Trying to set index for unknown validator"
            ),
            Some(mut v) => {
                let index = ValidatorIndex(index as usize);
                let mut index_set = self
                    .validators_per_committee
                    .entry(v.cluster.committee_id())
                    .or_default();
                if let Some(old_idx) = v.metadata.index {
                    if old_idx != index {
                        error!(
                            ?validator_pubkey,
                            db=?old_idx,
                            got=?index,
                            "Inconsistent validator index - database corrupt?"
                        );
                        index_set.remove(&old_idx);
                    }
                }
                v.metadata.index = Some(index);
                index_set.insert(index);
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
            let block = match block {
                UnsignedBlock::Full(FullBlockContents::BlockContents(contents)) => {
                    self.decide_abstract_block(validator_pubkey, contents).await
                }
                UnsignedBlock::Full(FullBlockContents::Block(block)) => {
                    self.decide_abstract_block(validator_pubkey, block).await
                }
                UnsignedBlock::Blinded(block) => {
                    self.decide_abstract_block(validator_pubkey, block).await
                }
            }?;

            // yay - we agree! let's sign the block we agreed on
            match block {
                UnsignedBlock::Full(FullBlockContents::BlockContents(contents)) => {
                    self.sign_abstract_block(validator_pubkey, contents, current_slot)
                        .await
                }
                UnsignedBlock::Full(FullBlockContents::Block(block)) => {
                    self.sign_abstract_block(validator_pubkey, block, current_slot)
                        .await
                }
                UnsignedBlock::Blinded(block) => {
                    self.sign_abstract_block(validator_pubkey, block, current_slot)
                        .await
                }
            }
        };

        run_and_update_metrics(
            BLOCK_LOG_NAME,
            &validator_metrics::SIGNED_BLOCKS_TOTAL,
            future,
        )
        .await
    }

    async fn sign_attestation(
        &self,
        validator_pubkey: PublicKeyBytes,
        validator_committee_position: usize,
        attestation: &mut Attestation<E>,
        current_epoch: Epoch,
    ) -> Result<(), Error> {
        let future = async {
            // Make sure the target epoch is not higher than the current epoch to avoid potential
            // attacks.
            if attestation.data().target.epoch > current_epoch {
                return Err(Error::GreaterThanCurrentEpoch {
                    epoch: attestation.data().target.epoch,
                    current_epoch,
                });
            }

            let validator = self.validator(validator_pubkey)?;

            let timer =
                metrics::start_timer_vec(&metrics::CONSENSUS_TIMES, &[metrics::BEACON_VOTE]);
            let start_time = self.get_instant_in_slot(
                attestation.data().slot,
                Duration::from_secs(self.spec.seconds_per_slot) / 3,
            )?;
            let completed = self
                .qbft_manager
                .decide_instance(
                    CommitteeInstanceId {
                        committee: validator.cluster.committee_id(),
                        instance_height: attestation.data().slot.as_usize().into(),
                    },
                    BeaconVote {
                        block_root: attestation.data().beacon_block_root,
                        source: attestation.data().source,
                        target: attestation.data().target,
                    },
                    start_time,
                    &validator.cluster,
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

            if !self.disable_slashing_protection {
                convert_slashing_result(self.slashing_protection.check_and_insert_attestation(
                    &validator_pubkey,
                    attestation.data(),
                    domain_hash,
                ))?;
            }

            let signing_root = attestation.data().signing_root(domain_hash);
            let signature = self
                .collect_signature(
                    PartialSignatureKind::PostConsensus,
                    Role::Committee,
                    Some(data_hash),
                    validator,
                    signing_root,
                    attestation.data().slot,
                )
                .await?;
            attestation
                .add_signature(&signature, validator_committee_position)
                .map_err(Error::UnableToSignAttestation)?;

            Ok(())
        };

        run_and_update_metrics(
            ATTESTATION_LOG_NAME,
            &validator_metrics::SIGNED_ATTESTATIONS_TOTAL,
            future,
        )
        .await
    }

    async fn sign_validator_registration_data(
        &self,
        mut validator_registration_data: ValidatorRegistrationData,
    ) -> Result<SignedValidatorRegistrationData, Error> {
        let future = async {
            let domain_hash = self.spec.get_builder_domain();
            let signing_root = validator_registration_data.signing_root(domain_hash);

            // SSV always uses the start of the current epoch, so we need to convert to that
            let epoch = self
                .slot_clock
                .slot_of(Duration::from_secs(validator_registration_data.timestamp))
                .unwrap_or(self.spec.genesis_slot)
                .epoch(E::slots_per_epoch());
            let sign_slot = epoch.start_slot(E::slots_per_epoch());
            let validity_slot = epoch.end_slot(E::slots_per_epoch());
            if let Some(duration) = self.slot_clock.start_of(sign_slot) {
                validator_registration_data.timestamp = duration.as_secs();
            }

            let signature = self
                .collect_signature(
                    PartialSignatureKind::ValidatorRegistration,
                    Role::ValidatorRegistration,
                    None,
                    self.validator(validator_registration_data.pubkey)?,
                    signing_root,
                    validity_slot,
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
            let signing_epoch = aggregate.data().target.epoch;
            let validator = self.validator(validator_pubkey)?;

            let version = match &aggregate {
                Attestation::Base(_) => ForkName::Base.into(),
                Attestation::Electra(_) => ForkName::Electra.into(),
            };

            let message =
                AggregateAndProof::from_attestation(aggregator_index, aggregate, selection_proof);

            // first, we have to get to consensus
            let timer = metrics::start_timer_vec(
                &metrics::CONSENSUS_TIMES,
                &[metrics::AGGREGATE_AND_PROOF],
            );
            let start_time = self.get_instant_in_slot(
                message.aggregate().data().slot,
                Duration::from_secs(self.spec.seconds_per_slot) * 2 / 3,
            )?;
            let completed = self
                .qbft_manager
                .decide_instance(
                    ValidatorInstanceId {
                        validator: validator_pubkey,
                        duty: ValidatorDutyKind::Aggregator,
                        instance_height: message.aggregate().data().slot.as_usize().into(),
                    },
                    ValidatorConsensusData {
                        duty: ValidatorDuty {
                            r#type: BEACON_ROLE_AGGREGATOR,
                            pub_key: validator_pubkey,
                            slot: message.aggregate().data().slot,
                            validator_index: validator
                                .metadata
                                .index
                                .ok_or(SpecificError::MissingIndex)?,
                            committee_index: message.aggregate().data().index,
                            // TODO: it seems the below are not needed (anymore?)
                            // potentially related: https://github.com/sigp/anchor/issues/263
                            committee_length: 0,
                            committees_at_slot: 0,
                            validator_committee_index: 0,
                            validator_sync_committee_indices: Default::default(),
                        },
                        version,
                        data_ssz: message.as_ssz_bytes(),
                    },
                    start_time,
                    &validator.cluster,
                )
                .await
                .map_err(SpecificError::from)?;
            drop(timer);

            let data = match completed {
                Completed::TimedOut => return Err(Error::SpecificError(SpecificError::Timeout)),
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
                    None,
                    validator,
                    signing_root,
                    message.aggregate().get_slot(),
                )
                .await?;

            Ok(SignedAggregateAndProof::from_aggregate_and_proof(
                message, signature,
            ))
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

            // We do not want to spend too long on the selection proof. We will not produce an
            // aggregation anyway if the proof is not known at 2/3rds into the slot - so we abort
            // then.
            let delay = Duration::from_secs(self.spec.seconds_per_slot) * 2 / 3;

            let signature = self
                .timeout_within_slot(
                    slot,
                    delay,
                    self.collect_signature(
                        PartialSignatureKind::SelectionProofPartialSig,
                        Role::Aggregator,
                        None,
                        self.validator(validator_pubkey)?,
                        signing_root,
                        slot,
                    ),
                )
                .await?;
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

            // We do not want to spend too long on the selection proof. We will not produce an
            // aggregation anyway if the proof is not known at 2/3rds into the slot - so we abort
            // then.
            let delay = Duration::from_secs(self.spec.seconds_per_slot) * 2 / 3;

            let signature = self
                .timeout_within_slot(
                    slot,
                    delay,
                    self.collect_signature(
                        PartialSignatureKind::ContributionProofs,
                        Role::SyncCommittee,
                        None,
                        self.validator(*validator_pubkey)?,
                        signing_root,
                        slot,
                    ),
                )
                .await?;

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
            let validator = self.validator(*validator_pubkey)?;
            let metadata = self.get_slot_metadata(slot).await?;

            let timer =
                metrics::start_timer_vec(&metrics::CONSENSUS_TIMES, &[metrics::BEACON_VOTE]);
            let start_time = self
                .get_instant_in_slot(slot, Duration::from_secs(self.spec.seconds_per_slot) / 3)?;
            let completed = self
                .qbft_manager
                .decide_instance(
                    CommitteeInstanceId {
                        committee: validator.cluster.committee_id(),
                        instance_height: slot.as_usize().into(),
                    },
                    metadata.beacon_vote.clone(),
                    start_time,
                    &validator.cluster,
                )
                .await
                .map_err(SpecificError::from)?;
            drop(timer);

            let data = match completed {
                Completed::TimedOut => return Err(Error::SpecificError(SpecificError::Timeout)),
                Completed::Success(data) => data,
            };

            let domain = self.get_domain(epoch, Domain::SyncCommittee);
            let signing_root = data.block_root.signing_root(domain);
            let signature = self
                .collect_signature(
                    PartialSignatureKind::PostConsensus,
                    Role::Committee,
                    Some(data.hash()),
                    validator,
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

            let subcommittee_index = contribution.subcommittee_index;

            let signing_data = ContributionAndProofSigningData {
                contribution,
                selection_proof,
            };

            let validator = match self.validator(aggregator_pubkey) {
                Ok(cluster) => cluster,
                Err(_) => return Err(Error::UnknownPubkey(aggregator_pubkey)),
            };

            let metadata = self.get_slot_metadata(slot).await?;

            let signing_data = match metadata.multi_sync_aggregators.get(&aggregator_pubkey) {
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
            let start_time = self.get_instant_in_slot(
                slot,
                Duration::from_secs(self.spec.seconds_per_slot) * 2 / 3,
            )?;
            let completed = self
                .qbft_manager
                .decide_instance(
                    ValidatorInstanceId {
                        validator: aggregator_pubkey,
                        duty: ValidatorDutyKind::SyncCommitteeAggregator,
                        instance_height: slot.as_usize().into(),
                    },
                    ValidatorConsensusData {
                        duty: ValidatorDuty {
                            r#type: BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION,
                            pub_key: aggregator_pubkey,
                            slot,
                            validator_index: validator
                                .metadata
                                .index
                                .ok_or(SpecificError::MissingIndex)?,
                            committee_index: 0,
                            committee_length: 0,
                            committees_at_slot: 0,
                            validator_committee_index: aggregator_index,
                            validator_sync_committee_indices: Default::default(),
                        },
                        version: ForkName::Altair.into(),
                        data_ssz: data.as_ssz_bytes(),
                    },
                    start_time,
                    &validator.cluster,
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
                None,
                validator,
                signing_root,
                slot,
            )
            .await
            .map(|signature| SignedContributionAndProof { message, signature })
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
        self.validator(*pubkey).ok().map(|v| ProposalData {
            validator_index: v.metadata.index.map(|idx| *idx as u64),
            fee_recipient: Some(v.cluster.fee_recipient),
            gas_limit: self.gas_limit,
            builder_proposals: self.builder_proposals,
        })
    }
}

trait SignableBlock<E: EthSpec>: Debug + Encode {
    type Payload: AbstractExecPayload<E>;

    fn as_block(&self) -> BeaconBlockRef<E, Self::Payload>;
    fn to_signed_block(self, signature: Signature) -> SignedBlock<E>;
}

impl<E: EthSpec> SignableBlock<E> for BlockContents<E> {
    type Payload = FullPayload<E>;

    fn as_block(&self) -> BeaconBlockRef<E, Self::Payload> {
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

    fn as_block(&self) -> BeaconBlockRef<E, Self::Payload> {
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

    fn as_block(&self) -> BeaconBlockRef<E, Self::Payload> {
        self.to_ref()
    }

    fn to_signed_block(self, signature: Signature) -> SignedBlock<E> {
        SignedBlock::Blinded(Arc::new(SignedBlindedBeaconBlock::from_block(
            self, signature,
        )))
    }
}

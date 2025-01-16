pub mod sync_committee_service;

use dashmap::DashMap;
use futures::future::join_all;
use parking_lot::Mutex;
use qbft::Completed;
use qbft_manager::{
    CommitteeInstanceId, QbftError, QbftManager, ValidatorDutyKind, ValidatorInstanceId,
};
use safe_arith::{ArithError, SafeArith};
use signature_collector::{CollectionError, SignatureCollectorManager, SignatureRequest};
use slashing_protection::{NotSafe, Safe, SlashingDatabase};
use slot_clock::SlotClock;
use ssv_types::message::{
    BeaconVote, Contribution, DataSsz, ValidatorConsensusData, ValidatorDuty,
    BEACON_ROLE_AGGREGATOR, BEACON_ROLE_PROPOSER, BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION,
    DATA_VERSION_ALTAIR, DATA_VERSION_BELLATRIX, DATA_VERSION_CAPELLA, DATA_VERSION_DENEB,
    DATA_VERSION_PHASE0, DATA_VERSION_UNKNOWN,
};
use ssv_types::{Cluster, OperatorId, ValidatorMetadata};
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{error, info, warn};
use types::attestation::Attestation;
use types::beacon_block::BeaconBlock;
use types::graffiti::Graffiti;
use types::selection_proof::SelectionProof;
use types::signed_aggregate_and_proof::SignedAggregateAndProof;
use types::signed_beacon_block::SignedBeaconBlock;
use types::signed_contribution_and_proof::SignedContributionAndProof;
use types::signed_voluntary_exit::SignedVoluntaryExit;
use types::slot_epoch::{Epoch, Slot};
use types::sync_committee_contribution::SyncCommitteeContribution;
use types::sync_committee_message::SyncCommitteeMessage;
use types::sync_selection_proof::SyncSelectionProof;
use types::sync_subnet_id::SyncSubnetId;
use types::validator_registration_data::{
    SignedValidatorRegistrationData, ValidatorRegistrationData,
};
use types::voluntary_exit::VoluntaryExit;
use types::{
    AbstractExecPayload, Address, AggregateAndProof, ChainSpec, ContributionAndProof, Domain,
    EthSpec, Hash256, PublicKeyBytes, SecretKey, Signature, SignedRoot,
    SyncAggregatorSelectionData, VariableList,
};
use validator_store::{
    DoppelgangerStatus, Error as ValidatorStoreError, ProposalData, SignedBlock, UnsignedBlock,
    ValidatorStore,
};

/// Number of epochs of slashing protection history to keep.
///
/// This acts as a maximum safe-guard against clock drift.
const SLASHING_PROTECTION_HISTORY_EPOCHS: u64 = 512;

#[derive(Clone)]
struct InitializedCluster {
    cluster: Cluster,
    validator_metadata: ValidatorMetadata,
    decrypted_key_share: SecretKey,
}

pub struct AnchorValidatorStore<T: SlotClock + 'static, E: EthSpec> {
    clusters: DashMap<PublicKeyBytes, InitializedCluster>,
    signature_collector: Arc<SignatureCollectorManager>,
    qbft_manager: Arc<QbftManager<T, E>>,
    slashing_protection: SlashingDatabase,
    slashing_protection_last_prune: Mutex<Epoch>,
    spec: Arc<ChainSpec>,
    genesis_validators_root: Hash256,
    operator_id: OperatorId,
}

impl<T: SlotClock, E: EthSpec> AnchorValidatorStore<T, E> {
    pub fn new(
        signature_collector: Arc<SignatureCollectorManager>,
        qbft_manager: Arc<QbftManager<T, E>>,
        slashing_protection: SlashingDatabase,
        spec: Arc<ChainSpec>,
        genesis_validators_root: Hash256,
        operator_id: OperatorId,
    ) -> AnchorValidatorStore<T, E> {
        Self {
            clusters: DashMap::new(),
            signature_collector,
            qbft_manager,
            slashing_protection,
            slashing_protection_last_prune: Mutex::new(Epoch::new(0)),
            spec,
            genesis_validators_root,
            operator_id,
        }
    }

    pub fn add_cluster(
        &self,
        cluster: Cluster,
        validator_metadata: ValidatorMetadata,
        decrypted_key_share: SecretKey,
    ) -> Result<(), Error> {
        let pubkey_bytes = validator_metadata.public_key.compress();
        self.clusters.insert(
            pubkey_bytes,
            InitializedCluster {
                cluster,
                validator_metadata,
                decrypted_key_share,
            },
        );
        self.slashing_protection
            .register_validator(pubkey_bytes)
            .map_err(Error::Slashable)
    }

    fn cluster(&self, validator_pubkey: PublicKeyBytes) -> Result<InitializedCluster, Error> {
        self.clusters
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
        cluster: InitializedCluster,
        signing_root: Hash256,
    ) -> Result<Signature, Error> {
        let collector = self.signature_collector.sign_and_collect(
            SignatureRequest {
                cluster_id: cluster.cluster.cluster_id,
                signing_root,
                threshold: cluster
                    .cluster
                    .faulty
                    .safe_mul(2)
                    .and_then(|x| x.safe_add(1))
                    .map_err(SpecificError::from)?,
            },
            self.operator_id,
            cluster.decrypted_key_share,
        );
        Ok((*collector.await.map_err(SpecificError::from)?).clone())
    }

    async fn decide_abstract_block<
        P: AbstractExecPayload<E>,
        F: FnOnce(BeaconBlock<E, P>) -> DataSsz<E>,
    >(
        &self,
        validator_pubkey: PublicKeyBytes,
        block: BeaconBlock<E, P>,
        current_slot: Slot,
        wrapper: F,
    ) -> Result<DataSsz<E>, Error> {
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

        let cluster = self.cluster(validator_pubkey)?;

        // first, we have to get to consensus
        let completed = self
            .qbft_manager
            .decide_instance(
                ValidatorInstanceId {
                    validator: validator_pubkey,
                    duty: ValidatorDutyKind::Proposal,
                    instance_height: block.slot().as_usize().into(),
                },
                ValidatorConsensusData {
                    duty: ValidatorDuty {
                        r#type: BEACON_ROLE_PROPOSER,
                        pub_key: validator_pubkey,
                        slot: block.slot().as_usize().into(),
                        validator_index: cluster.validator_metadata.index,
                        committee_index: 0,
                        committee_length: 0,
                        committees_at_slot: 0,
                        validator_committee_index: 0,
                        validator_sync_committee_indices: Default::default(),
                    },
                    version: match &block {
                        BeaconBlock::Base(_) => DATA_VERSION_PHASE0,
                        BeaconBlock::Altair(_) => DATA_VERSION_ALTAIR,
                        BeaconBlock::Bellatrix(_) => DATA_VERSION_BELLATRIX,
                        BeaconBlock::Capella(_) => DATA_VERSION_CAPELLA,
                        BeaconBlock::Deneb(_) => DATA_VERSION_DENEB,
                        BeaconBlock::Electra(_) => DATA_VERSION_UNKNOWN,
                    },
                    data_ssz: Box::new(wrapper(block)),
                },
                &cluster.cluster,
            )
            .await
            .map_err(SpecificError::from)?;
        let data = match completed {
            Completed::TimedOut => return Err(Error::SpecificError(SpecificError::Timeout)),
            Completed::Success(data) => data,
        };
        Ok(*data.data_ssz)
    }

    async fn sign_abstract_block<P: AbstractExecPayload<E>>(
        &self,
        validator_pubkey: PublicKeyBytes,
        block: BeaconBlock<E, P>,
    ) -> Result<SignedBeaconBlock<E, P>, Error> {
        let domain_hash = self.get_domain(block.epoch(), Domain::BeaconProposer);

        let header = block.block_header();
        handle_slashing_check_result(
            self.slashing_protection.check_and_insert_block_proposal(
                &validator_pubkey,
                &header,
                domain_hash,
            ),
            &header,
            "block",
        )?;

        let signing_root = block.signing_root(domain_hash);
        let signature = self
            .collect_signature(self.cluster(validator_pubkey)?, signing_root)
            .await?;
        Ok(SignedBeaconBlock::from_block(block, signature))
    }

    pub async fn produce_sync_committee_signature_with_full_vote(
        &self,
        slot: Slot,
        vote: BeaconVote,
        validator_index: u64,
        validator_pubkey: &PublicKeyBytes,
    ) -> Result<SyncCommitteeMessage, Error> {
        let epoch = slot.epoch(E::slots_per_epoch());
        let cluster = self.cluster(*validator_pubkey)?;
        let beacon_block_root = vote.block_root;

        let completed = self
            .qbft_manager
            .decide_instance(
                CommitteeInstanceId {
                    committee: cluster.cluster.cluster_id,
                    instance_height: slot.as_usize().into(),
                },
                vote,
                &cluster.cluster,
            )
            .await
            .map_err(SpecificError::from)?;
        let data = match completed {
            Completed::TimedOut => return Err(Error::SpecificError(SpecificError::Timeout)),
            Completed::Success(data) => data,
        };

        let domain = self.get_domain(epoch, Domain::SyncCommittee);
        let signing_root = data.block_root.signing_root(domain);
        let signature = self.collect_signature(cluster, signing_root).await?;

        Ok(SyncCommitteeMessage {
            slot,
            beacon_block_root,
            validator_index,
            signature,
        })
    }

    pub async fn produce_signed_contribution_and_proofs(
        &self,
        aggregator_index: u64,
        aggregator_pubkey: PublicKeyBytes,
        signing_data: Vec<ContributionAndProofSigningData<E>>,
    ) -> Vec<Result<SignedContributionAndProof<E>, Error>> {
        let error = |err: Error| signing_data.iter().map(move |_| Err(err.clone())).collect();

        let Some(slot) = signing_data.first().map(|data| data.contribution.slot) else {
            return vec![];
        };
        let epoch = slot.epoch(E::slots_per_epoch());
        let cluster = match self.cluster(aggregator_pubkey) {
            Ok(cluster) => cluster,
            Err(err) => return error(err),
        };

        let data = match VariableList::new(
            signing_data
                .iter()
                .map(|signing_data| Contribution {
                    selection_proof_sig: signing_data.selection_proof.clone().into(),
                    contribution: signing_data.contribution.clone(),
                })
                .collect(),
        ) {
            Ok(data) => data,
            Err(_) => return error(SpecificError::TooManySyncSubnetsToSign.into()),
        };

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
                        validator_index: cluster.validator_metadata.index,
                        committee_index: 0,
                        committee_length: 0,
                        committees_at_slot: 0,
                        validator_committee_index: aggregator_index,
                        validator_sync_committee_indices: Default::default(),
                    },
                    version: DATA_VERSION_PHASE0,
                    data_ssz: Box::new(DataSsz::Contributions(data)),
                },
                &cluster.cluster,
            )
            .await;
        let data = match completed {
            Ok(Completed::Success(data)) => data,
            Ok(Completed::TimedOut) => return error(SpecificError::Timeout.into()),
            Err(err) => return error(SpecificError::QbftError(err).into()),
        };
        let data = match *data.data_ssz {
            DataSsz::Contributions(data) => data,
            _ => return error(SpecificError::InvalidQbftData.into()),
        };

        let domain_hash = self.get_domain(epoch, Domain::ContributionAndProof);
        let signing_futures = data
            .into_iter()
            .map(|contribution| {
                let cluster = cluster.clone();
                async move {
                    let message = ContributionAndProof {
                        aggregator_index,
                        contribution: contribution.contribution,
                        selection_proof: contribution.selection_proof_sig,
                    };
                    let signing_root = message.signing_root(domain_hash);
                    self.collect_signature(cluster, signing_root)
                        .await
                        .map(|signature| SignedContributionAndProof { message, signature })
                }
            })
            .collect::<Vec<_>>();

        join_all(signing_futures).await
    }
}

fn handle_slashing_check_result(
    slashing_status: Result<Safe, NotSafe>,
    object: impl Debug,
    kind: &'static str,
) -> Result<(), Error> {
    match slashing_status {
        // We can safely sign this attestation.
        Ok(Safe::Valid) => Ok(()),
        Ok(Safe::SameData) => {
            warn!("Skipping signing of previously signed {kind}",);
            validator_metrics::inc_counter_vec(
                &validator_metrics::SIGNED_ATTESTATIONS_TOTAL,
                &[validator_metrics::SAME_DATA],
            );
            Err(Error::SameData)
        }
        Err(NotSafe::UnregisteredValidator(pk)) => {
            error!(
                "public_key" = format!("{:?}", pk),
                "Internal error: validator was not properly registered for slashing protection",
            );
            validator_metrics::inc_counter_vec(
                &validator_metrics::SIGNED_ATTESTATIONS_TOTAL,
                &[validator_metrics::UNREGISTERED],
            );
            Err(Error::Slashable(NotSafe::UnregisteredValidator(pk)))
        }
        Err(e) => {
            error!(
                "object" = format!("{:?}", object),
                "error" = format!("{:?}", e),
                "Not signing slashable {kind}",
            );
            validator_metrics::inc_counter_vec(
                &validator_metrics::SIGNED_ATTESTATIONS_TOTAL,
                &[validator_metrics::SLASHABLE],
            );
            Err(Error::Slashable(e))
        }
    }
}

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
    InvalidQbftData,
    TooManySyncSubnetsToSign,
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

pub type Error = ValidatorStoreError<SpecificError>;

impl<T: SlotClock, E: EthSpec> ValidatorStore for AnchorValidatorStore<T, E> {
    type Error = SpecificError;
    type E = E;

    fn validator_index(&self, pubkey: &PublicKeyBytes) -> Option<u64> {
        self.clusters
            .get(pubkey)
            .map(|v| v.validator_metadata.index.0 as u64)
    }

    fn voting_pubkeys<I, F>(&self, _filter_func: F) -> I
    where
        I: FromIterator<PublicKeyBytes>,
        F: Fn(DoppelgangerStatus) -> Option<PublicKeyBytes>,
    {
        // we don't care about doppelgangers
        self.clusters.iter().map(|v| *v.key()).collect()
    }

    fn doppelganger_protection_allows_signing(&self, _validator_pubkey: PublicKeyBytes) -> bool {
        // we don't care about doppelgangers
        true
    }

    fn num_voting_validators(&self) -> usize {
        self.clusters.len()
    }

    fn graffiti(&self, validator_pubkey: &PublicKeyBytes) -> Option<Graffiti> {
        self.clusters
            .get(validator_pubkey)
            .map(|v| v.validator_metadata.graffiti)
    }

    fn get_fee_recipient(&self, validator_pubkey: &PublicKeyBytes) -> Option<Address> {
        self.clusters
            .get(validator_pubkey)
            .map(|v| v.cluster.fee_recipient)
    }

    fn determine_builder_boost_factor(&self, _validator_pubkey: &PublicKeyBytes) -> Option<u64> {
        Some(1)
    }

    async fn randao_reveal(
        &self,
        validator_pubkey: PublicKeyBytes,
        signing_epoch: Epoch,
    ) -> Result<Signature, Error> {
        let domain_hash = self.get_domain(signing_epoch, Domain::Randao);
        let signing_root = signing_epoch.signing_root(domain_hash);
        self.collect_signature(self.cluster(validator_pubkey)?, signing_root)
            .await
    }

    fn set_validator_index(&self, validator_pubkey: &PublicKeyBytes, index: u64) {
        // we actually have the index already. we use the opportunity to do a sanity check
        match self.clusters.get(validator_pubkey) {
            None => warn!(
                validator = validator_pubkey.as_hex_string(),
                "Trying to set index for unknown validator"
            ),
            Some(v) => {
                if v.validator_metadata.index.0 as u64 != index {
                    error!(
                        validator = validator_pubkey.as_hex_string(),
                        expected = v.validator_metadata.index.0,
                        actual = index,
                        "Mismatched validator index",
                    )
                }
            }
        }
    }

    async fn sign_block(
        &self,
        validator_pubkey: PublicKeyBytes,
        block: UnsignedBlock<E>,
        current_slot: Slot,
    ) -> Result<SignedBlock<E>, Error> {
        let data = match block {
            UnsignedBlock::Full(block) => {
                self.decide_abstract_block(
                    validator_pubkey,
                    block,
                    current_slot,
                    DataSsz::BeaconBlock,
                )
                .await
            }
            UnsignedBlock::Blinded(block) => {
                self.decide_abstract_block(
                    validator_pubkey,
                    block,
                    current_slot,
                    DataSsz::BlindedBeaconBlock,
                )
                .await
            }
        }?;

        // yay - we agree! let's sign the block we agreed on
        match data {
            DataSsz::BeaconBlock(block) => Ok(self
                .sign_abstract_block(validator_pubkey, block)
                .await?
                .into()),
            DataSsz::BlindedBeaconBlock(block) => Ok(self
                .sign_abstract_block(validator_pubkey, block)
                .await?
                .into()),
            _ => Err(Error::SpecificError(SpecificError::InvalidQbftData)),
        }
    }

    async fn sign_attestation(
        &self,
        validator_pubkey: PublicKeyBytes,
        validator_committee_position: usize,
        attestation: &mut Attestation<E>,
        current_epoch: Epoch,
    ) -> Result<(), Error> {
        // Make sure the target epoch is not higher than the current epoch to avoid potential attacks.
        if attestation.data().target.epoch > current_epoch {
            return Err(Error::GreaterThanCurrentEpoch {
                epoch: attestation.data().target.epoch,
                current_epoch,
            });
        }

        let cluster = self.cluster(validator_pubkey)?;

        let completed = self
            .qbft_manager
            .decide_instance(
                CommitteeInstanceId {
                    committee: cluster.cluster.cluster_id,
                    instance_height: attestation.data().slot.as_usize().into(),
                },
                BeaconVote {
                    block_root: attestation.data().beacon_block_root,
                    source: attestation.data().source,
                    target: attestation.data().target,
                },
                &cluster.cluster,
            )
            .await
            .map_err(SpecificError::from)?;
        let data = match completed {
            Completed::TimedOut => return Err(Error::SpecificError(SpecificError::Timeout)),
            Completed::Success(data) => data,
        };
        attestation.data_mut().beacon_block_root = data.block_root;
        attestation.data_mut().source = data.source;
        attestation.data_mut().target = data.target;

        // yay - we agree! let's sign the att we agreed on
        let domain_hash = self.get_domain(current_epoch, Domain::BeaconAttester);

        handle_slashing_check_result(
            self.slashing_protection.check_and_insert_attestation(
                &validator_pubkey,
                attestation.data(),
                domain_hash,
            ),
            attestation.data(),
            "attestation",
        )?;

        let signing_root = attestation.data().signing_root(domain_hash);
        let signature = self.collect_signature(cluster, signing_root).await?;
        attestation
            .add_signature(&signature, validator_committee_position)
            .map_err(Error::UnableToSignAttestation)?;

        Ok(())
    }

    async fn sign_voluntary_exit(
        &self,
        _validator_pubkey: PublicKeyBytes,
        _voluntary_exit: VoluntaryExit,
    ) -> Result<SignedVoluntaryExit, Error> {
        // there should be no situation ever where we want to sign an exit
        Err(Error::SpecificError(SpecificError::Unsupported))
    }

    async fn sign_validator_registration_data(
        &self,
        validator_registration_data: ValidatorRegistrationData,
    ) -> Result<SignedValidatorRegistrationData, Error> {
        let domain_hash = self.spec.get_builder_domain();
        let signing_root = validator_registration_data.signing_root(domain_hash);

        let signature = self
            .collect_signature(
                self.cluster(validator_registration_data.pubkey)?,
                signing_root,
            )
            .await?;

        Ok(SignedValidatorRegistrationData {
            message: validator_registration_data,
            signature,
        })
    }

    async fn produce_signed_aggregate_and_proof(
        &self,
        validator_pubkey: PublicKeyBytes,
        aggregator_index: u64,
        aggregate: Attestation<E>,
        selection_proof: SelectionProof,
    ) -> Result<SignedAggregateAndProof<E>, Error> {
        let signing_epoch = aggregate.data().target.epoch;
        let cluster = self.cluster(validator_pubkey)?;

        let message =
            AggregateAndProof::from_attestation(aggregator_index, aggregate, selection_proof);

        // first, we have to get to consensus
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
                        validator_index: cluster.validator_metadata.index,
                        committee_index: message.aggregate().data().index,
                        // todo it seems the below are not needed (anymore?)
                        committee_length: 0,
                        committees_at_slot: 0,
                        validator_committee_index: 0,
                        validator_sync_committee_indices: Default::default(),
                    },
                    version: DATA_VERSION_PHASE0,
                    data_ssz: Box::new(DataSsz::AggregateAndProof(message)),
                },
                &cluster.cluster,
            )
            .await
            .map_err(SpecificError::from)?;
        let data = match completed {
            Completed::TimedOut => return Err(Error::SpecificError(SpecificError::Timeout)),
            Completed::Success(data) => data,
        };
        let message = match *data.data_ssz {
            DataSsz::AggregateAndProof(message) => message,
            _ => return Err(Error::SpecificError(SpecificError::InvalidQbftData)),
        };

        let domain_hash = self.get_domain(signing_epoch, Domain::AggregateAndProof);
        let signing_root = message.signing_root(domain_hash);
        let signature = self.collect_signature(cluster, signing_root).await?;

        Ok(SignedAggregateAndProof::from_aggregate_and_proof(
            message, signature,
        ))
    }

    async fn produce_selection_proof(
        &self,
        validator_pubkey: PublicKeyBytes,
        slot: Slot,
    ) -> Result<SelectionProof, Error> {
        let epoch = slot.epoch(E::slots_per_epoch());
        let domain_hash = self.get_domain(epoch, Domain::SelectionProof);
        let signing_root = slot.signing_root(domain_hash);

        self.collect_signature(self.cluster(validator_pubkey)?, signing_root)
            .await
            .map(SelectionProof::from)
    }

    async fn produce_sync_selection_proof(
        &self,
        validator_pubkey: &PublicKeyBytes,
        slot: Slot,
        subnet_id: SyncSubnetId,
    ) -> Result<SyncSelectionProof, Error> {
        let epoch = slot.epoch(E::slots_per_epoch());
        let domain_hash = self.get_domain(epoch, Domain::SyncCommitteeSelectionProof);
        let signing_root = SyncAggregatorSelectionData {
            slot,
            subcommittee_index: subnet_id.into(),
        }
        .signing_root(domain_hash);

        self.collect_signature(self.cluster(*validator_pubkey)?, signing_root)
            .await
            .map(SyncSelectionProof::from)
    }

    async fn produce_sync_committee_signature(
        &self,
        _slot: Slot,
        _beacon_block_root: Hash256,
        _validator_index: u64,
        _validator_pubkey: &PublicKeyBytes,
    ) -> Result<SyncCommitteeMessage, Error> {
        // use `produce_sync_committee_signature_with_full_vote` instead
        Err(Error::SpecificError(SpecificError::Unsupported))
    }

    async fn produce_signed_contribution_and_proof(
        &self,
        _aggregator_index: u64,
        _aggregator_pubkey: PublicKeyBytes,
        _contribution: SyncCommitteeContribution<E>,
        _selection_proof: SyncSelectionProof,
    ) -> Result<SignedContributionAndProof<E>, Error> {
        // use `produce_signed_contribution_and_proofs` instead
        Err(Error::SpecificError(SpecificError::Unsupported))
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
        self.clusters.get(pubkey).map(|v| ProposalData {
            validator_index: Some(v.validator_metadata.index.0 as u64),
            fee_recipient: Some(v.cluster.fee_recipient),
            gas_limit: 29_999_998,    // TODO support scalooors
            builder_proposals: false, // TODO support MEVooors
        })
    }
}

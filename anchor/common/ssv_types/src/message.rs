use crate::msgid::MsgId;
use crate::{OperatorId, ValidatorIndex};
use tree_hash::{PackedEncoding, TreeHash, TreeHashType};
use tree_hash_derive::TreeHash;
use types::typenum::U13;
use types::{
    AggregateAndProof, BeaconBlock, BlindedBeaconBlock, Checkpoint, CommitteeIndex, EthSpec,
    Hash256, PublicKeyBytes, Signature, Slot, SyncCommitteeContribution, VariableList,
};
// todo - dear reader, this mainly serves as plain translation of the types found in the go code
// there are a lot of byte[] there, and that got confusing, below should be more readable.
// it needs some work to actually serialize to the same stuff on wire, and I feel like we can name
// the fields better

#[derive(Clone, Debug)]
pub struct SignedSsvMessage<E: EthSpec> {
    pub signatures: Vec<[u8; 256]>,
    pub operator_ids: Vec<OperatorId>,
    pub ssv_message: SsvMessage<E>,
    pub full_data: Option<FullData<E>>,
}

#[derive(Clone, Debug)]
pub struct SsvMessage<E: EthSpec> {
    pub msg_type: MsgType,
    pub msg_id: MsgId,
    pub data: Data<E>,
}

#[derive(Clone, Debug)]
pub enum MsgType {
    SsvConsensusMsgType,
    SsvPartialSignatureMsgType,
}

#[derive(Clone, Debug)]
pub enum Data<E: EthSpec> {
    QbftMessage(QbftMessage<E>),
    PartialSignatureMessage(PartialSignatureMessage),
}

#[derive(Clone, Debug)]
pub struct QbftMessage<E: EthSpec> {
    pub qbft_message_type: QbftMessageType,
    pub height: u64,
    pub round: u64,
    pub identifier: MsgId,

    pub root: Hash256,
    pub round_change_justification: Vec<SignedSsvMessage<E>>, // always without full_data
    pub prepare_justification: Vec<SignedSsvMessage<E>>,      // always without full_data
}

#[derive(Clone, Debug)]
pub enum QbftMessageType {
    ProposalMsgType,
    PrepareMsgType,
    CommitMsgType,
    RoundChangeMsgType,
}

#[derive(Clone, Debug)]
pub struct PartialSignatureMessage {
    pub partial_signature: Signature,
    pub signing_root: Hash256,
    pub signer: OperatorId,
    pub validator_index: ValidatorIndex,
}

#[derive(Clone, Debug)]
pub enum FullData<E: EthSpec> {
    ValidatorConsensusData(ValidatorConsensusData<E>),
    BeaconVote(BeaconVote),
}

#[derive(Clone, Debug, TreeHash)]
pub struct ValidatorConsensusData<E: EthSpec> {
    pub duty: ValidatorDuty,
    pub version: DataVersion,
    pub data_ssz: Box<DataSsz<E>>,
}

impl<E: EthSpec> qbft::Data for ValidatorConsensusData<E> {
    type Hash = Hash256;

    fn hash(&self) -> Self::Hash {
        self.tree_hash_root()
    }
}

#[derive(Clone, Debug, TreeHash)]
pub struct ValidatorDuty {
    pub r#type: BeaconRole,
    pub pub_key: PublicKeyBytes,
    pub slot: Slot,
    pub validator_index: ValidatorIndex,
    pub committee_index: CommitteeIndex,
    pub committee_length: u64,
    pub committees_at_slot: u64,
    pub validator_committee_index: u64,
    pub validator_sync_committee_indices: VariableList<u64, U13>,
}

#[derive(Clone, Debug)]
pub struct BeaconRole(u64);

pub const BEACON_ROLE_ATTESTER: BeaconRole = BeaconRole(0);
pub const BEACON_ROLE_AGGREGATOR: BeaconRole = BeaconRole(1);
pub const BEACON_ROLE_PROPOSER: BeaconRole = BeaconRole(2);
pub const BEACON_ROLE_SYNC_COMMITTEE: BeaconRole = BeaconRole(3);
pub const BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION: BeaconRole = BeaconRole(4);
pub const BEACON_ROLE_VALIDATOR_REGISTRATION: BeaconRole = BeaconRole(5);
pub const BEACON_ROLE_VOLUNTARY_EXIT: BeaconRole = BeaconRole(6);
pub const BEACON_ROLE_UNKNOWN: BeaconRole = BeaconRole(u64::MAX);

impl TreeHash for BeaconRole {
    fn tree_hash_type() -> TreeHashType {
        u64::tree_hash_type()
    }

    fn tree_hash_packed_encoding(&self) -> PackedEncoding {
        self.0.tree_hash_packed_encoding()
    }

    fn tree_hash_packing_factor() -> usize {
        u64::tree_hash_packing_factor()
    }

    fn tree_hash_root(&self) -> tree_hash::Hash256 {
        self.0.tree_hash_root()
    }
}

#[derive(Clone, Debug)]
pub struct DataVersion(u64);

pub const DATA_VERSION_UNKNOWN: DataVersion = DataVersion(0);
pub const DATA_VERSION_PHASE0: DataVersion = DataVersion(1);
pub const DATA_VERSION_ALTAIR: DataVersion = DataVersion(2);
pub const DATA_VERSION_BELLATRIX: DataVersion = DataVersion(3);
pub const DATA_VERSION_CAPELLA: DataVersion = DataVersion(4);
pub const DATA_VERSION_DENEB: DataVersion = DataVersion(5);

impl TreeHash for DataVersion {
    fn tree_hash_type() -> TreeHashType {
        u64::tree_hash_type()
    }

    fn tree_hash_packed_encoding(&self) -> PackedEncoding {
        self.0.tree_hash_packed_encoding()
    }

    fn tree_hash_packing_factor() -> usize {
        u64::tree_hash_packing_factor()
    }

    fn tree_hash_root(&self) -> tree_hash::Hash256 {
        self.0.tree_hash_root()
    }
}

#[derive(Clone, Debug, TreeHash)]
#[tree_hash(enum_behaviour = "transparent")]
pub enum DataSsz<E: EthSpec> {
    AggregateAndProof(AggregateAndProof<E>),
    BlindedBeaconBlock(BlindedBeaconBlock<E>),
    BeaconBlock(BeaconBlock<E>),
    Contributions(VariableList<Contribution<E>, U13>),
}

#[derive(Clone, Debug, TreeHash)]
pub struct Contribution<E: EthSpec> {
    pub selection_proof_sig: Signature,
    pub contribution: SyncCommitteeContribution<E>,
}

#[derive(Clone, Debug, TreeHash)]
pub struct BeaconVote {
    pub block_root: Hash256,
    pub source: Checkpoint,
    pub target: Checkpoint,
}

impl qbft::Data for BeaconVote {
    type Hash = Hash256;

    fn hash(&self) -> Self::Hash {
        self.tree_hash_root()
    }
}

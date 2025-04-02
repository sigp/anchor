use proptest::prelude::*;
use ssv_types::consensus::{BeaconVote, QbftMessage, QbftMessageType};
use ssv_types::domain_type::DomainType;
use ssv_types::message::{MsgType, SSVMessage, SignedSSVMessage, RSA_SIGNATURE_SIZE};
use ssv_types::msgid::{DutyExecutor, MessageId, Role};
use ssv_types::partial_sig::{
    PartialSignatureKind, PartialSignatureMessage, PartialSignatureMessages,
};
use ssv_types::{CommitteeId, CommitteeInfo, IndexSet, OperatorId, ValidatorIndex};
use ssz::Encode;
use types::{Checkpoint, Hash256, PublicKeyBytes, Signature, Slot};

// Builder for QbftMessages
#[derive(Debug, Clone)]
pub struct QbftMessageBuilder {
    msg_type: QbftMessageType,
    height: u64,
    round: u64,
    identifier: MessageId,
    root: Hash256,
    data_round: u64,
    prepare_justification: Vec<SignedSSVMessage>,
    round_change_justification: Vec<SignedSSVMessage>,
}

impl QbftMessageBuilder {
    /// Create a new builder with default values
    pub fn new(role: Role, msg_type: QbftMessageType) -> Self {
        Self {
            msg_type,
            height: 1,
            round: 1,
            identifier: create_message_id(role),
            root: Hash256::ZERO,
            data_round: 1,
            prepare_justification: vec![],
            round_change_justification: vec![],
        }
    }

    /// Set the round value
    pub fn with_round(mut self, round: u64) -> Self {
        self.round = round;
        self
    }

    /// Set the height value
    pub fn with_height(mut self, height: u64) -> Self {
        self.height = height;
        self
    }

    /// Set the message ID
    pub fn with_identifier(mut self, identifier: MessageId) -> Self {
        self.identifier = identifier;
        self
    }

    /// Set the root hash
    pub fn with_root(mut self, root: Hash256) -> Self {
        self.root = root;
        self
    }

    /// Set the data round
    pub fn with_data_round(mut self, data_round: u64) -> Self {
        self.data_round = data_round;
        self
    }

    /// Set prepare justifications
    pub fn with_prepare_justification(mut self, justifications: Vec<SignedSSVMessage>) -> Self {
        self.prepare_justification = justifications;
        self
    }

    /// Set round change justifications
    pub fn with_round_change_justification(
        mut self,
        justifications: Vec<SignedSSVMessage>,
    ) -> Self {
        self.round_change_justification = justifications;
        self
    }

    /// Build the QbftMessage
    pub fn build(self) -> QbftMessage {
        QbftMessage {
            qbft_message_type: self.msg_type,
            height: self.height,
            round: self.round,
            identifier: (&self.identifier).into(),
            root: self.root,
            data_round: self.data_round,
            round_change_justification: self.round_change_justification,
            prepare_justification: self.prepare_justification,
        }
    }
}

/// Builder for SignedSSVMessage to simplify test creation
pub struct SignedMessageBuilder {
    qbft_message: Option<QbftMessage>,
    partial_sig_message: Option<PartialSignatureMessages>,
    signers: Vec<OperatorId>,
    full_data: Vec<u8>,
}

impl SignedMessageBuilder {
    /// Create a new builder for consensus messages
    pub fn consensus(qbft_message: QbftMessage) -> Self {
        Self {
            qbft_message: Some(qbft_message),
            partial_sig_message: None,
            signers: vec![],
            full_data: vec![],
        }
    }

    /// Create a new builder for partial signature messages
    pub fn partial_signature(partial_sig_message: PartialSignatureMessages) -> Self {
        Self {
            qbft_message: None,
            partial_sig_message: Some(partial_sig_message),
            signers: vec![],
            full_data: vec![],
        }
    }

    /// Set the signers for the message
    pub fn with_signers(mut self, signers: Vec<OperatorId>) -> Self {
        self.signers = signers;
        self
    }

    /// Set the full data for the message
    pub fn with_full_data(mut self, full_data: Vec<u8>) -> Self {
        self.full_data = full_data;
        self
    }

    /// Build the SignedSSVMessage
    pub fn build(self) -> Result<SignedSSVMessage, String> {
        // Validate signers
        if self.signers.is_empty() {
            return Err("Must provide at least one signer".to_string());
        }
        if self.signers.iter().any(|s| s.0 == 0) {
            return Err("OperatorId(0) is not allowed as it causes ZeroSigner error".to_string());
        }

        let mut signers = self.signers.clone();
        signers.sort();

        if self.qbft_message.is_some() {
            self.build_consensus_message(signers)
        } else if self.partial_sig_message.is_some() {
            self.build_partial_sig_message(signers)
        } else {
            Err("Either QbftMessage or PartialSignatureMessages must be provided".to_string())
        }
    }

    fn build_consensus_message(
        &self,
        signers: Vec<OperatorId>,
    ) -> Result<SignedSSVMessage, String> {
        let qbft_message = self.qbft_message.as_ref().unwrap();
        let qbft_bytes = qbft_message.as_ssz_bytes();

        // Extract message ID from identifier
        let slice: &[u8] = qbft_message.identifier.as_ref();
        let msg_id: [u8; 56] = slice
            .try_into()
            .map_err(|_| "VariableList does not contain exactly 56 bytes".to_string())?;

        let ssv_msg = SSVMessage::new(MsgType::SSVConsensusMsgType, msg_id.into(), qbft_bytes)
            .map_err(|e| format!("Failed to create SSVMessage: {:?}", e))?;

        let signatures = generate_signatures(signers.len());

        SignedSSVMessage::new(signatures, signers, ssv_msg, self.full_data.clone())
            .map_err(|e| format!("Failed to create SignedSSVMessage: {:?}", e))
    }

    fn build_partial_sig_message(
        &self,
        signers: Vec<OperatorId>,
    ) -> Result<SignedSSVMessage, String> {
        let partial_sig_message = self.partial_sig_message.as_ref().unwrap();
        let encoded = partial_sig_message.as_ssz_bytes();

        // Get role based on the partial signature kind
        let role = match partial_sig_message.kind {
            PartialSignatureKind::PostConsensus => Role::Committee,
            PartialSignatureKind::RandaoPartialSig => Role::Proposer,
            PartialSignatureKind::SelectionProofPartialSig => Role::Aggregator,
            PartialSignatureKind::ContributionProofs => Role::SyncCommittee,
            PartialSignatureKind::ValidatorRegistration => Role::ValidatorRegistration,
            PartialSignatureKind::VoluntaryExit => Role::VoluntaryExit,
        };

        let msg_id = create_message_id(role);

        let ssv_msg = SSVMessage::new(MsgType::SSVPartialSignatureMsgType, msg_id, encoded)
            .map_err(|e| format!("Failed to create SSVMessage: {:?}", e))?;

        let signatures = generate_signatures(signers.len());

        SignedSSVMessage::new(signatures, signers, ssv_msg, self.full_data.clone())
            .map_err(|e| format!("Failed to create SignedSSVMessage: {:?}", e))
    }
}

/// Builder for partial signature messages
pub struct PartialSignatureBuilder {
    kind: PartialSignatureKind,
    slot: u64,
    signer: OperatorId,
    messages: Vec<PartialSignatureMessage>,
}

impl PartialSignatureBuilder {
    /// Create a new builder with default values
    pub fn new(kind: PartialSignatureKind, slot: u64) -> Self {
        Self {
            kind,
            slot,
            signer: OperatorId(1),
            messages: vec![],
        }
    }

    /// Set the signer for the partial signature messages
    pub fn with_signer(mut self, signer: OperatorId) -> Self {
        self.signer = signer;
        self
    }

    /// Add a message to the partial signature messages
    pub fn add_message(mut self, signing_root: Hash256, validator_index: ValidatorIndex) -> Self {
        self.messages.push(PartialSignatureMessage {
            partial_signature: Signature::empty(),
            signing_root,
            signer: self.signer,
            validator_index,
        });
        self
    }

    /// Build the PartialSignatureMessages
    pub fn build(self) -> PartialSignatureMessages {
        PartialSignatureMessages {
            kind: self.kind,
            slot: Slot::new(self.slot),
            messages: self.messages,
        }
    }
}

/// Builder for BeaconVote
pub struct BeaconVoteBuilder {
    block_root: Hash256,
    source: Checkpoint,
    target: Checkpoint,
}

impl BeaconVoteBuilder {
    /// Create a new builder with default values
    pub fn new() -> Self {
        Self {
            block_root: Hash256::ZERO,
            source: Checkpoint::default(),
            target: Checkpoint::default(),
        }
    }

    /// Set the block root
    pub fn with_block_root(mut self, block_root: Hash256) -> Self {
        self.block_root = block_root;
        self
    }

    /// Set the source checkpoint
    pub fn with_source(mut self, source: Checkpoint) -> Self {
        self.source = source;
        self
    }

    /// Set the target checkpoint
    pub fn with_target(mut self, target: Checkpoint) -> Self {
        self.target = target;
        self
    }

    /// Build the BeaconVote
    pub fn build(self) -> BeaconVote {
        BeaconVote {
            block_root: self.block_root,
            source: self.source,
            target: self.target,
        }
    }
}

/// Creates a standard test DomainType
pub fn test_domain() -> DomainType {
    DomainType([0, 0, 0, 1])
}

/// Creates a message ID for testing with the specified role
pub fn create_message_id(role: Role) -> MessageId {
    let domain = test_domain();
    let duty_executor = match role {
        Role::Committee => DutyExecutor::Committee(CommitteeId([0u8; 32])),
        _ => DutyExecutor::Validator(PublicKeyBytes::empty()),
    };
    MessageId::new(&domain, role, &duty_executor)
}

/// Generate valid signatures of specified length
pub fn generate_signatures(count: usize) -> Vec<Vec<u8>> {
    (0..count)
        .map(|i| vec![0xAA + i as u8; RSA_SIGNATURE_SIZE])
        .collect()
}

/// Generate valid operator IDs (non-zero, sorted)
pub fn generate_operator_ids(count: usize) -> Vec<OperatorId> {
    (1..=count as u64).map(OperatorId).collect()
}

// Strategy for generating QbftMessageType
pub fn qbft_message_type_strategy() -> impl Strategy<Value = QbftMessageType> {
    prop_oneof![
        Just(QbftMessageType::Proposal),
        Just(QbftMessageType::Prepare),
        Just(QbftMessageType::Commit),
        Just(QbftMessageType::RoundChange),
    ]
}

// Strategy for generating PartialSignatureKind
pub fn partial_signature_kind_strategy() -> impl Strategy<Value = PartialSignatureKind> {
    prop_oneof![
        Just(PartialSignatureKind::PostConsensus),
        Just(PartialSignatureKind::RandaoPartialSig),
        Just(PartialSignatureKind::SelectionProofPartialSig),
        Just(PartialSignatureKind::ContributionProofs),
        Just(PartialSignatureKind::ValidatorRegistration),
        Just(PartialSignatureKind::VoluntaryExit),
    ]
}

// Strategy for generating Role
pub fn role_strategy() -> impl Strategy<Value = Role> {
    prop_oneof![
        Just(Role::Committee),
        Just(Role::Proposer),
        Just(Role::Aggregator),
        Just(Role::SyncCommittee),
        Just(Role::ValidatorRegistration),
        Just(Role::VoluntaryExit),
    ]
}

#[cfg(test)]
mod fuzz_validation_tests {
    use super::*;

    proptest! {

        #[test]
        fn test_message_building(role in role_strategy(), msg_type in qbft_message_type_strategy()) {
            let beacon_vote = BeaconVoteBuilder::new().build();
            let qbft_message = QbftMessageBuilder::new(role, msg_type).build();
            let signed_message = SignedMessageBuilder::consensus(qbft_message).build();
            println!("{:?}", signed_message);

        }
    }
}

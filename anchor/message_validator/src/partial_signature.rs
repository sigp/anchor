use std::{collections::HashMap, sync::Arc};

use duties_tracker::DutiesProvider;
use slot_clock::SlotClock;
use ssv_types::{
    OperatorId,
    msgid::Role,
    partial_sig::{PartialSignatureKind, PartialSignatureMessages, PartialSignatureMessagesError},
};
use ssz::Decode;
use types::consts::altair::SYNC_COMMITTEE_SUBNET_COUNT;

use crate::{
    ValidatedSSVMessage, ValidationContext, ValidationFailure, duty_state::DutyState,
    validate_beacon_duty, validate_duty_count, validate_role_for_fork, validate_slot_time,
    verify_message_signature,
};

// Constants for validation rules
const MAX_SIGNATURES_IN_SYNC_COMMITTEE: usize = 13;

pub(crate) fn validate_partial_signature_message(
    validation_context: ValidationContext<impl SlotClock>,
    duty_state: &mut DutyState,
    duty_provider: Arc<impl DutiesProvider>,
) -> Result<ValidatedSSVMessage, ValidationFailure> {
    // Decode message directly to PartialSignatureMessages
    let messages = match PartialSignatureMessages::from_ssz_bytes(
        validation_context.signed_ssv_message.ssv_message().data(),
    ) {
        Ok(msgs) => msgs,
        Err(err) => return Err(ValidationFailure::UndecodableMessageData(err)),
    };

    // Validate role is allowed for the fork active at this slot
    validate_role_for_fork(messages.slot, &validation_context)?;

    // Validate basic semantics
    let signer = validate_partial_signature_message_semantics(&validation_context, &messages)?;

    // Validate duty-specific logic
    validate_partial_sig_messages_by_duty_logic(
        &validation_context,
        &messages,
        duty_state,
        duty_provider,
    )?;

    let operator_pub_keys = validation_context.operator_pub_keys.get(&signer).ok_or(
        ValidationFailure::OperatorNotFound {
            operator_id: signer,
        },
    )?;

    let signature = validation_context
        .signed_ssv_message
        .signatures()
        .first()
        .ok_or(ValidationFailure::NoSignatures)?;

    verify_message_signature(
        validation_context.signed_ssv_message,
        operator_pub_keys,
        signature,
    )?;

    // Update the duty state with information about this partial signature message
    let signer = validation_context
        .signed_ssv_message
        .operator_ids()
        .first()
        .ok_or(ValidationFailure::NoSigners)?;

    duty_state.update_for_partial_signature(
        &messages,
        signer,
        validation_context.slots_per_epoch,
    )?;

    Ok(ValidatedSSVMessage::PartialSignatureMessages(messages))
}

fn validate_partial_signature_message_semantics(
    validation_context: &ValidationContext<impl SlotClock>,
    partial_signature_messages: &PartialSignatureMessages,
) -> Result<OperatorId, ValidationFailure> {
    // Rule: Partial Signature message must have 1 signer
    let signers = validation_context.signed_ssv_message.operator_ids();
    if signers.len() != 1 {
        return Err(ValidationFailure::PartialSigOneSigner);
    }

    let signer = signers[0];

    // Rule: Partial signature message must not have full data
    if !validation_context.signed_ssv_message.full_data().is_empty() {
        return Err(ValidationFailure::FullDataNotInConsensusMessage);
    }

    // Rule: Partial signature type must match expected type for role
    if !partial_signature_type_matches_role(
        partial_signature_messages.kind,
        validation_context.role,
    ) {
        return Err(ValidationFailure::PartialSignatureTypeRoleMismatch);
    }

    // Structural validation: empty, internal signer consistency, zero signer.
    let inner_signer = partial_signature_messages.validate().map_err(|e| match e {
        PartialSignatureMessagesError::Empty => ValidationFailure::NoPartialSignatureMessages,
        PartialSignatureMessagesError::InconsistentSigners => {
            ValidationFailure::InconsistentSigners
        }
        PartialSignatureMessagesError::ZeroSigner => ValidationFailure::ZeroSigner,
    })?;

    // Rule: Partial signature signer must match the signed message's signer.
    if inner_signer != signer {
        return Err(ValidationFailure::InconsistentSigners);
    }

    // Validate validator indices for non-committee duties
    for message in &partial_signature_messages.messages {
        // Rule: (only for Validator duties) Validator index must match with validatorPK
        // For Committee duties (Committee and AggregatorCommittee), we don't assume that
        // operators are synced on the validators set, so we skip this check.
        // This allows batched messages to contain validators from multiple operators' committees.
        if !validation_context.role.is_committee_role()
            && !validation_context
                .committee_info
                .validator_indices
                .is_empty()
            && !validation_context
                .committee_info
                .validator_indices
                .contains(&message.validator_index)
        {
            return Err(ValidationFailure::ValidatorIndexMismatch);
        }
    }

    Ok(signer)
}

fn partial_signature_type_matches_role(kind: PartialSignatureKind, role: Role) -> bool {
    match role {
        Role::Committee => kind == PartialSignatureKind::PostConsensus,
        Role::PTCAttester => kind == PartialSignatureKind::PTCAttester,
        Role::ProposerPreferences => kind == PartialSignatureKind::ProposerPreferences,
        Role::Aggregator => {
            kind == PartialSignatureKind::PostConsensus
                || kind == PartialSignatureKind::SelectionProofPartialSig
        }
        Role::Proposer => {
            kind == PartialSignatureKind::PostConsensus
                || kind == PartialSignatureKind::RandaoPartialSig
        }
        Role::SyncCommittee => {
            kind == PartialSignatureKind::PostConsensus
                || kind == PartialSignatureKind::ContributionProofs
        }
        Role::ValidatorRegistration => kind == PartialSignatureKind::ValidatorRegistration,
        Role::VoluntaryExit => kind == PartialSignatureKind::VoluntaryExit,
        Role::AggregatorCommittee => {
            kind == PartialSignatureKind::PostConsensus
                || kind == PartialSignatureKind::AggregatorCommitteePartialSig
        }
    }
}

/// Validates partial signature messages based on duty logic.
fn validate_partial_sig_messages_by_duty_logic(
    validation_context: &ValidationContext<impl SlotClock>,
    partial_signature_messages: &PartialSignatureMessages,
    duty_state: &mut DutyState,
    duty_provider: Arc<impl DutiesProvider>,
) -> Result<(), ValidationFailure> {
    let role = validation_context.role;
    let message_slot = partial_signature_messages.slot;
    let signed_message = validation_context.signed_ssv_message;

    // Get the operator ID (signer)
    let signer = signed_message
        .operator_ids()
        .first()
        .ok_or(ValidationFailure::NoSigners)?;

    // Get duty state for this signer
    let operator_state = duty_state.get_or_create_operator(signer);

    // Rule: Slot must not be "old" - signer must not have already advanced to a later slot
    // Skip for committee roles (Committee and AggregatorCommittee)
    if !role.is_committee_role() {
        let max_slot = operator_state.max_slot();
        if max_slot.as_u64() != 0 && max_slot > message_slot {
            return Err(ValidationFailure::SlotAlreadyAdvanced {
                got: message_slot.as_u64(),
                want: max_slot.as_u64(),
            });
        }
    }

    let is_randao_msg = partial_signature_messages.kind == PartialSignatureKind::RandaoPartialSig;
    validate_beacon_duty(
        validation_context,
        message_slot,
        is_randao_msg,
        duty_provider.clone(),
    )?;

    // Check if we've seen messages for this slot already
    if let Some(signer_state) = operator_state.get_signer_state(&message_slot) {
        // Rule: peer must send only:
        // - 1 PostConsensusPartialSig, for Committee duty
        // - 1 RandaoPartialSig and 1 PostConsensusPartialSig for Proposer
        // - 1 SelectionProofPartialSig and 1 PostConsensusPartialSig for Aggregator
        // - 1 SelectionProofPartialSig and 1 PostConsensusPartialSig for Sync committee
        //   contribution
        // - 1 ValidatorRegistrationPartialSig for Validator Registration
        // - 1 VoluntaryExitPartialSig for Voluntary Exit
        signer_state
            .message_counts
            .validate_partial_signature_message(partial_signature_messages)?;
    }

    // Check timing constraints
    validate_slot_time(message_slot, validation_context)?;

    // Validate duty count
    validate_duty_count(
        validation_context,
        message_slot,
        operator_state,
        duty_provider.clone(),
    )?;

    // Process role-specific message count constraints
    // Safety: validator_count is bounded by SSV committee limits (max ~3000 validators per
    // committee), so multiplications like 2*V or 5*V cannot overflow usize.
    let validator_count = validation_context.committee_info.validator_indices.len();
    let message_count = partial_signature_messages.messages.len();

    match role {
        Role::Committee => {
            // Rule: Number of signatures must be <= min(2*V, V + SYNC_COMMITTEE_SIZE)
            let max_allowed = std::cmp::min(
                2 * validator_count,
                validator_count + validation_context.sync_committee_size,
            );

            if message_count > max_allowed {
                return Err(ValidationFailure::TooManyPartialSignatureMessages {
                    got: message_count,
                    limit: max_allowed,
                });
            }

            // Rule: A validator index can't appear more than 2 times
            let mut validator_index_count = HashMap::new();
            for message in &partial_signature_messages.messages {
                let count = validator_index_count
                    .entry(message.validator_index)
                    .or_insert(0);
                *count += 1;
                if *count > 2 {
                    return Err(ValidationFailure::TooManyValidatorIndexOccurrences {
                        validator_index: message.validator_index,
                        got: *count,
                        limit: 2,
                    });
                }
            }
        }
        Role::SyncCommittee => {
            if message_count > MAX_SIGNATURES_IN_SYNC_COMMITTEE {
                // Rule: Number of signatures must be <= MAX_SIGNATURES_IN_SYNC_COMMITTEE
                return Err(ValidationFailure::TooManyPartialSignatureMessages {
                    got: message_count,
                    limit: MAX_SIGNATURES_IN_SYNC_COMMITTEE,
                });
            }
        }
        Role::AggregatorCommittee => {
            // Formula: min(5*V, V + 4*sync_committee_size) where V = validator count.
            // Rationale: Each validator can produce 1 attestation + 4 sync messages = 5 max.
            // For large committees (V > sync_committee_size), the global sync committee size
            // caps it at V + 4*sync_committee_size. Examples: V=100 → 500, V=1000 → 3048.
            // Note: Kind validation already done by partial_signature_type_matches_role()
            let max_allowed = std::cmp::min(
                5 * validator_count,
                validator_count
                    + (SYNC_COMMITTEE_SUBNET_COUNT as usize
                        * validation_context.sync_committee_size),
            );
            if message_count > max_allowed {
                return Err(ValidationFailure::TooManyPartialSignatureMessages {
                    got: message_count,
                    limit: max_allowed,
                });
            }

            // Rule: A validator index can't appear more than 5 times
            // (1 attestation + 4 sync committee subnets = 5 max)
            let mut validator_index_count = HashMap::new();
            for message in &partial_signature_messages.messages {
                let count = validator_index_count
                    .entry(message.validator_index)
                    .or_insert(0);
                *count += 1;
                if *count > 5 {
                    return Err(ValidationFailure::TooManyValidatorIndexOccurrences {
                        validator_index: message.validator_index,
                        got: *count,
                        limit: 5,
                    });
                }
            }
        }
        // Per-validator roles only allow one signature
        Role::Aggregator
        | Role::Proposer
        | Role::ValidatorRegistration
        | Role::VoluntaryExit
        | Role::PTCAttester
        | Role::ProposerPreferences => {
            if message_count > 1 {
                return Err(ValidationFailure::TooManyPartialSignatureMessages {
                    got: message_count,
                    limit: 1,
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use bls::{Hash256, Signature};
    use fork::{Fork, ForkSchedule};
    use openssl::{
        hash::MessageDigest,
        pkey::{PKey, Private, Public},
        rsa::Rsa,
        sign::Signer,
    };
    use slot_clock::{ManualSlotClock, SlotClock};
    use ssv_types::{
        OperatorId, RSA_SIGNATURE_SIZE, ValidatorIndex, VariableList,
        domain_type::DomainType,
        message::{MsgType, SSVMessage, SignedSSVMessage},
        partial_sig::PartialSignatureMessage,
    };
    use ssz::Encode;
    use types::{EthSpec, MainnetEthSpec, Slot};

    use super::*;
    use crate::tests::{
        FOUR_NODE_COMMITTEE, MockDutiesProvider, assert_validation_error, create_committee_info,
        create_message_id_for_test, create_operator_pub_keys, generate_random_rsa_public_keys,
    };

    // Options for creating test partial signature messages
    #[derive(Default)]
    pub struct PartialSigTestOptions {
        pub add_full_data: bool,
        pub different_message_signer: Option<OperatorId>,
        pub empty_messages: bool,
        pub validator_index: Option<ValidatorIndex>,
    }

    // Helper to create a partial signature message for testing
    pub fn create_test_partial_signature(
        role: Role,
        kind: PartialSignatureKind,
        signer: OperatorId,
        options: PartialSigTestOptions,
        operator_pk: Option<Rsa<Private>>,
    ) -> (PartialSignatureMessages, SignedSSVMessage) {
        let message_signer = options.different_message_signer.unwrap_or(signer);

        let messages = if options.empty_messages {
            vec![]
        } else {
            vec![PartialSignatureMessage {
                partial_signature: Signature::empty(),
                signing_root: Hash256::from([0u8; 32]),
                signer: message_signer,
                validator_index: options.validator_index.unwrap_or(ValidatorIndex(0)),
            }]
        };

        let partial_sig_messages = PartialSignatureMessages {
            kind,
            slot: Slot::new(0),
            messages: VariableList::new(messages).unwrap(),
        };

        let msg_id = create_message_id_for_test(role);
        let ssv_msg_data = partial_sig_messages.as_ssz_bytes();
        let ssv_msg = SSVMessage::new(MsgType::SSVPartialSignatureMsgType, msg_id, ssv_msg_data)
            .expect("SSVMessage should be created");

        let full_data = if options.add_full_data {
            vec![0xCC; 32]
        } else {
            vec![]
        };

        let signature = if let Some(pk) = operator_pk {
            let p_key = PKey::from_rsa(pk.clone()).unwrap();
            let mut signer = Signer::new(MessageDigest::sha256(), &p_key).unwrap();
            signer.update(&ssv_msg.as_ssz_bytes()).unwrap();
            vec![
                signer
                    .sign_to_vec()
                    .expect("Failed to sign message")
                    .try_into()
                    .expect("Signature should be 256 bytes"),
            ]
        } else {
            vec![[0xAA; RSA_SIGNATURE_SIZE]]
        };

        let signed_msg = SignedSSVMessage::new(signature, vec![signer], ssv_msg, full_data)
            .expect("SignedSSVMessage should be created");

        (partial_sig_messages, signed_msg)
    }

    // Import helper function from consensus_message tests or redefine here
    fn generate_test_key_pair() -> (Rsa<Private>, Rsa<Public>) {
        let private_key = Rsa::generate(2048).expect("Failed to generate RSA key");
        let public_key = Rsa::from_public_components(
            private_key.n().to_owned().unwrap(),
            private_key.e().to_owned().unwrap(),
        )
        .expect("Failed to extract public key");
        (private_key, public_key)
    }

    // Helper function to create a ValidationContext for testing
    fn create_test_validation_context<'a>(
        signed_msg: &'a SignedSSVMessage,
        committee_info: &'a crate::CommitteeInfo,
        role: Role,
        operator_pub_keys: &'a HashMap<OperatorId, Rsa<Public>>,
        fork_schedule: Arc<ForkSchedule>,
    ) -> ValidationContext<'a, ManualSlotClock> {
        create_test_validation_context_with_fork(
            signed_msg,
            committee_info,
            role,
            operator_pub_keys,
            Some(fork_schedule),
        )
    }

    // Helper function to create a ValidationContext with custom fork schedule
    fn create_test_validation_context_with_fork<'a>(
        signed_msg: &'a SignedSSVMessage,
        committee_info: &'a crate::CommitteeInfo,
        role: Role,
        operator_pub_keys: &'a HashMap<OperatorId, Rsa<Public>>,
        fork_schedule: Option<Arc<ForkSchedule>>,
    ) -> ValidationContext<'a, ManualSlotClock> {
        let fork_schedule = fork_schedule.unwrap_or_else(|| generate_fork_schedule(Fork::Alan));
        ValidationContext {
            signed_ssv_message: signed_msg,
            committee_info,
            role,
            received_at: SystemTime::now(),
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock: ManualSlotClock::new(
                Slot::new(0),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap(),
                Duration::from_secs(1),
            ),
            operator_pub_keys,
            fork_schedule,
            spec: spec_with_gloas(None),
        }
    }

    fn generate_fork_schedule(fork: Fork) -> Arc<ForkSchedule> {
        Arc::new(ForkSchedule::new(fork, DomainType::default(), "testing"))
    }

    /// Build a `ChainSpec` whose Ethereum Gloas (ePBS) fork activates at
    /// `gloas_fork_epoch` (`None` = "Gloas never happens"). Used to gate
    /// post-Gloas roles such as `PTCAttester`.
    fn spec_with_gloas(gloas_fork_epoch: Option<u64>) -> Arc<types::ChainSpec> {
        let mut spec = types::ChainSpec::mainnet();
        spec.gloas_fork_epoch = gloas_fork_epoch.map(types::Epoch::new);
        Arc::new(spec)
    }

    #[test]
    fn test_aggregator_committee_message_count_small_committee() {
        // Small committee (V ≤ 512): min(5*V, V + 4*512) = 5*V
        // V=10: min(50, 2058) = 50
        let validator_count = 10;
        let max_allowed = 5 * validator_count; // 50

        // Should accept exactly the limit
        let result = validate_aggregator_committee_message_count(max_allowed, validator_count);
        assert!(result.is_ok());

        // Should reject one over the limit
        let result = validate_aggregator_committee_message_count(max_allowed + 1, validator_count);
        assert!(result.is_err());
    }

    #[test]
    fn test_aggregator_committee_message_count_large_committee() {
        // Large committee (V > 512): min(5*V, V + 4*512) = V + 2048
        // V=1000: min(5000, 3048) = 3048
        let validator_count = 1000;
        let max_allowed = validator_count + (4 * 512); // 3048

        // Should accept exactly the limit
        let result = validate_aggregator_committee_message_count(max_allowed, validator_count);
        assert!(result.is_ok());

        // Should reject one over the limit
        let result = validate_aggregator_committee_message_count(max_allowed + 1, validator_count);
        assert!(result.is_err());
    }

    #[test]
    fn test_aggregator_committee_message_count_boundary() {
        // Boundary case (V = 512): min(5*512, 512 + 4*512) = min(2560, 2560) = 2560
        let validator_count = 512;
        let max_allowed = 5 * validator_count; // 2560 (both formulas equal here)

        // Should accept exactly the limit
        let result = validate_aggregator_committee_message_count(max_allowed, validator_count);
        assert!(result.is_ok());

        // Should reject one over the limit
        let result = validate_aggregator_committee_message_count(max_allowed + 1, validator_count);
        assert!(result.is_err());
    }

    // Helper function for message count testing (kind validation done upstream)
    fn validate_aggregator_committee_message_count(
        message_count: usize,
        validator_count: usize,
    ) -> Result<(), ValidationFailure> {
        let sync_committee_size = MainnetEthSpec::sync_committee_size();
        let max_allowed = std::cmp::min(
            5 * validator_count,
            validator_count + (SYNC_COMMITTEE_SUBNET_COUNT as usize * sync_committee_size),
        );
        if message_count > max_allowed {
            return Err(ValidationFailure::TooManyPartialSignatureMessages {
                got: message_count,
                limit: max_allowed,
            });
        }
        Ok(())
    }

    #[test]
    fn test_partial_signature_message_with_invalid_type_for_role() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        let (_, signed_msg) = create_test_partial_signature(
            Role::Committee,
            PartialSignatureKind::RandaoPartialSig, // Invalid for Committee role
            OperatorId(1),
            PartialSigTestOptions::default(),
            None,
        );

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let fork_schedule = ForkSchedule::new(Fork::Alan, DomainType::default(), "testing");
        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::Committee,
            &map,
            Arc::new(fork_schedule),
        );

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::PartialSignatureTypeRoleMismatch),
            "PartialSignatureTypeRoleMismatch",
        );
    }

    #[test]
    fn test_partial_signature_message_with_multiple_signers() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        let (messages, _) = create_test_partial_signature(
            Role::Proposer,
            PartialSignatureKind::RandaoPartialSig,
            OperatorId(1),
            PartialSigTestOptions::default(),
            None,
        );

        // Create a new SignedSSVMessage with multiple signers
        let ssv_msg_data = messages.as_ssz_bytes();
        let msg_id = create_message_id_for_test(Role::Proposer);
        let ssv_msg = SSVMessage::new(MsgType::SSVPartialSignatureMsgType, msg_id, ssv_msg_data)
            .expect("SSVMessage should be created");

        // Multiple signers - this should fail
        let signers = vec![OperatorId(1), OperatorId(2)];
        let signatures = vec![[0xAA; RSA_SIGNATURE_SIZE], [0xBB; RSA_SIGNATURE_SIZE]];

        let signed_msg = SignedSSVMessage::new(signatures, signers, ssv_msg, vec![])
            .expect("SignedSSVMessage should be created");

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let fork_schedule = ForkSchedule::new(Fork::Alan, DomainType::default(), "testing");
        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::Proposer,
            &map,
            Arc::new(fork_schedule),
        );

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::PartialSigOneSigner),
            "PartialSigOneSigner",
        );
    }

    #[test]
    fn test_partial_signature_message_with_full_data() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        let (_, signed_msg) = create_test_partial_signature(
            Role::Proposer,
            PartialSignatureKind::RandaoPartialSig,
            OperatorId(1),
            PartialSigTestOptions {
                add_full_data: true,
                ..Default::default()
            },
            None,
        );

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let fork_schedule = ForkSchedule::new(Fork::Alan, DomainType::default(), "testing");
        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::Proposer,
            &map,
            Arc::new(fork_schedule),
        );

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::FullDataNotInConsensusMessage),
            "FullDataNotInConsensusMessage",
        );
    }

    #[test]
    fn test_partial_signature_message_inconsistent_signers() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        let (_, signed_msg) = create_test_partial_signature(
            Role::Proposer,
            PartialSignatureKind::RandaoPartialSig,
            OperatorId(1),
            PartialSigTestOptions {
                different_message_signer: Some(OperatorId(42)),
                ..Default::default()
            },
            None,
        );

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let fork_schedule = ForkSchedule::new(Fork::Alan, DomainType::default(), "testing");
        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::Proposer,
            &map,
            Arc::new(fork_schedule),
        );

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::InconsistentSigners),
            "InconsistentSigners",
        );
    }

    #[test]
    fn test_partial_signature_message_no_messages() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        let (_, signed_msg) = create_test_partial_signature(
            Role::Proposer,
            PartialSignatureKind::RandaoPartialSig,
            OperatorId(1),
            PartialSigTestOptions {
                empty_messages: true,
                ..Default::default()
            },
            None,
        );

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let fork_schedule = ForkSchedule::new(Fork::Alan, DomainType::default(), "testing");
        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::Proposer,
            &map,
            Arc::new(fork_schedule),
        );

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::NoPartialSignatureMessages),
            "NoPartialSignatureMessages",
        );
    }

    #[test]
    fn test_partial_signature_message_successful() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();

        let (_, signed_msg) = create_test_partial_signature(
            Role::Proposer,
            PartialSignatureKind::RandaoPartialSig,
            OperatorId(1),
            PartialSigTestOptions::default(),
            Some(private_key),
        );

        let binding = [public_key];
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), binding.to_vec());

        let fork_schedule = ForkSchedule::new(Fork::Alan, DomainType::default(), "testing");
        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::Proposer,
            &map,
            Arc::new(fork_schedule),
        );

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert!(
            result.is_ok(),
            "{}",
            format!("Expected successful validation but got: {result:?}")
        );

        if let Ok(ValidatedSSVMessage::PartialSignatureMessages(messages)) = result {
            assert_eq!(messages.kind, PartialSignatureKind::RandaoPartialSig);
            assert_eq!(messages.messages.len(), 1);
            assert_eq!(messages.messages[0].signer, OperatorId(1));
        } else {
            panic!("Expected PartialSignatureMessages in successful validation");
        }
    }

    #[test]
    fn test_validator_index_mismatch() {
        // Create committee info with specific validator indices
        let mut committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        committee_info.validator_indices = vec![ValidatorIndex(10), ValidatorIndex(20)];

        let (_, signed_msg) = create_test_partial_signature(
            Role::Proposer,
            PartialSignatureKind::RandaoPartialSig,
            OperatorId(1),
            PartialSigTestOptions {
                validator_index: Some(ValidatorIndex(30)), // Not in committee
                ..Default::default()
            },
            None,
        );

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let fork_schedule = ForkSchedule::new(Fork::Alan, DomainType::default(), "testing");
        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::Proposer, // Not a committee role, so validator index is checked
            &map,
            Arc::new(fork_schedule),
        );

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::ValidatorIndexMismatch),
            "ValidatorIndexMismatch",
        );
    }

    #[test]
    fn test_committee_role_skips_validator_index_check() {
        // Create committee info with specific validator indices
        let mut committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        committee_info.validator_indices = vec![ValidatorIndex(10), ValidatorIndex(20)];

        let (private_key, public_key) = generate_test_key_pair();

        let (_, signed_msg) = create_test_partial_signature(
            Role::Committee,
            PartialSignatureKind::PostConsensus, // Valid for Committee role
            OperatorId(1),
            PartialSigTestOptions {
                validator_index: Some(ValidatorIndex(30)), /* Not in committee, but ignored for
                                                            * Committee role */
                ..Default::default()
            },
            Some(private_key),
        );

        let binding = [public_key];
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), binding.to_vec());

        let fork_schedule = ForkSchedule::new(Fork::Alan, DomainType::default(), "testing");
        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::Committee, // Committee role, so validator index is not checked
            &map,
            Arc::new(fork_schedule),
        );

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert!(
            result.is_ok(),
            "{}",
            format!("Expected successful validation for Committee role, but got: {result:?}")
        );
    }

    #[test]
    fn test_aggregator_committee_role_skips_validator_index_check() {
        // Create committee info with specific validator indices
        let mut committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        committee_info.validator_indices = vec![ValidatorIndex(10), ValidatorIndex(20)];

        let (private_key, public_key) = generate_test_key_pair();

        let (_, signed_msg) = create_test_partial_signature(
            Role::AggregatorCommittee,
            PartialSignatureKind::AggregatorCommitteePartialSig,
            OperatorId(1),
            PartialSigTestOptions {
                validator_index: Some(ValidatorIndex(30)), /* Not in committee, but ignored for
                                                            * AggregatorCommittee role */
                ..Default::default()
            },
            Some(private_key),
        );

        let binding = [public_key];
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), binding.to_vec());

        // Create fork schedule with Boole at epoch 0 (active from start)
        let fork_schedule = generate_fork_schedule(Fork::Boole);

        let validation_context = create_test_validation_context_with_fork(
            &signed_msg,
            &committee_info,
            Role::AggregatorCommittee, /* AggregatorCommittee role, so validator index is not
                                        * checked */
            &map,
            Some(fork_schedule),
        );

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert!(
            result.is_ok(),
            "{}",
            format!(
                "Expected successful validation for AggregatorCommittee role with unknown validator index, but got: {result:?}"
            )
        );
    }

    fn create_partial_signature_messages_with_count(count: usize) -> Vec<PartialSignatureMessage> {
        let mut messages = vec![];
        for _ in 0..count {
            messages.push(PartialSignatureMessage {
                partial_signature: Signature::empty(),
                signing_root: Hash256::from([0u8; 32]),
                signer: OperatorId(1),
                validator_index: ValidatorIndex(0),
            });
        }
        messages
    }

    /// Helper function to validate sync committee partial signatures with a given message count.
    /// Returns the validation result for assertion in individual tests.
    fn validate_sync_committee_signature_count(
        message_count: usize,
    ) -> Result<ValidatedSSVMessage, ValidationFailure> {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        let messages = create_partial_signature_messages_with_count(message_count);

        let partial_sig_messages = PartialSignatureMessages {
            kind: PartialSignatureKind::PostConsensus,
            slot: Slot::new(0),
            messages: VariableList::new(messages).unwrap(),
        };

        let msg_id = create_message_id_for_test(Role::SyncCommittee);
        let ssv_msg_data = partial_sig_messages.as_ssz_bytes();
        let ssv_msg = SSVMessage::new(MsgType::SSVPartialSignatureMsgType, msg_id, ssv_msg_data)
            .expect("SSVMessage should be created");

        let (private_key, public_key) = generate_test_key_pair();
        let p_key = PKey::from_rsa(private_key).unwrap();
        let mut signer = Signer::new(MessageDigest::sha256(), &p_key).unwrap();
        signer.update(&ssv_msg.as_ssz_bytes()).unwrap();
        let signature = vec![
            signer
                .sign_to_vec()
                .expect("Failed to sign message")
                .try_into()
                .expect("Signature should be 256 bytes"),
        ];

        let signed_msg = SignedSSVMessage::new(signature, vec![OperatorId(1)], ssv_msg, vec![])
            .expect("SignedSSVMessage should be created");

        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);

        let fork_schedule = ForkSchedule::new(Fork::Alan, DomainType::default(), "testing");
        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::SyncCommittee,
            &map,
            Arc::new(fork_schedule),
        );

        validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        )
    }

    #[test]
    fn test_too_many_partial_signature_messages() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        // Create messages with more than allowed count
        let messages = create_partial_signature_messages_with_count(3);

        let partial_sig_messages = PartialSignatureMessages {
            kind: PartialSignatureKind::PostConsensus,
            slot: Slot::new(0),
            messages: VariableList::new(messages).unwrap(),
        };

        let msg_id = create_message_id_for_test(Role::Proposer); // Not committee role
        let ssv_msg_data = partial_sig_messages.as_ssz_bytes();
        let ssv_msg = SSVMessage::new(MsgType::SSVPartialSignatureMsgType, msg_id, ssv_msg_data)
            .expect("SSVMessage should be created");

        let signed_msg = SignedSSVMessage::new(
            vec![[0xAA; RSA_SIGNATURE_SIZE]],
            vec![OperatorId(1)],
            ssv_msg,
            vec![],
        )
        .expect("SignedSSVMessage should be created");

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let fork_schedule = ForkSchedule::new(Fork::Alan, DomainType::default(), "testing");
        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::Proposer,
            &map,
            Arc::new(fork_schedule),
        );

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::TooManyPartialSignatureMessages { .. }
                )
            },
            "TooManyPartialSignatureMessages",
        );
    }

    #[test]
    fn test_sync_committee_accepts_multiple_signatures_within_limit() {
        // Test 3 signatures (well within the limit)
        let result = validate_sync_committee_signature_count(3);

        assert!(
            result.is_ok(),
            "{}",
            format!("Expected successful validation but got: {result:?}")
        );
    }

    #[test]
    fn test_sync_committee_accepts_max_signatures() {
        // Test exactly 13 signatures (at the limit)
        let result = validate_sync_committee_signature_count(13);

        assert!(
            result.is_ok(),
            "{}",
            format!("Expected successful validation for 13 signatures but got: {result:?}")
        );
    }

    #[test]
    fn test_sync_committee_rejects_too_many_signatures() {
        // Test 14 signatures (one over the limit)
        let result = validate_sync_committee_signature_count(14);

        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::TooManyPartialSignatureMessages { got: 14, limit: 13 }
                )
            },
            "TooManyPartialSignatureMessages with got=14, limit=13",
        );
    }

    #[test]
    fn test_sync_committee_accepts_single_signature() {
        // Test 1 signature (minimal case)
        let result = validate_sync_committee_signature_count(1);

        assert!(
            result.is_ok(),
            "{}",
            format!("Expected successful validation for single signature but got: {result:?}")
        );
    }

    #[test]
    fn test_committee_validator_index_exceeds_limit() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);

        // Create messages with a validator index that appears 3 times (exceeds limit of 2)
        let messages = create_partial_signature_messages_with_count(3);

        let partial_sig_messages = PartialSignatureMessages {
            kind: PartialSignatureKind::PostConsensus,
            slot: Slot::new(0),
            messages: VariableList::new(messages).unwrap(),
        };

        let msg_id = create_message_id_for_test(Role::Committee);
        let ssv_msg_data = partial_sig_messages.as_ssz_bytes();
        let ssv_msg = SSVMessage::new(MsgType::SSVPartialSignatureMsgType, msg_id, ssv_msg_data)
            .expect("SSVMessage should be created");

        let signed_msg = SignedSSVMessage::new(
            vec![[0xAA; RSA_SIGNATURE_SIZE]],
            vec![OperatorId(1)],
            ssv_msg,
            vec![],
        )
        .expect("SignedSSVMessage should be created");

        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);

        let fork_schedule = ForkSchedule::new(Fork::Alan, DomainType::default(), "testing");
        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::Committee,
            &map,
            Arc::new(fork_schedule),
        );

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::TooManyValidatorIndexOccurrences { limit: 2, .. }
                )
            },
            "TooManyValidatorIndexOccurrences",
        );
    }

    // TTL test constants
    const SLOTS_PER_EPOCH_TEST: u64 = 32;
    const LATE_SLOT_ALLOWANCE_TEST: u64 = 2;
    const TTL_SLOTS: u64 = SLOTS_PER_EPOCH_TEST + LATE_SLOT_ALLOWANCE_TEST; // 34 slots
    const BEYOND_TTL_SLOTS: u64 = 40;
    // Past the short slot-bound TTL (1 + LATE_SLOT_ALLOWANCE_TEST = 3 slots),
    // well within the committee TTL (TTL_SLOTS = 34). Lets a test prove which
    // TTL bucket a role is in, rather than just probing a boundary.
    const COMMITTEE_TTL_BUCKET_SLOTS: u64 = 20;

    // Helper to create validation context for TTL tests
    fn create_ttl_validation_context<'a>(
        signed_msg: &'a SignedSSVMessage,
        committee_info: &'a crate::CommitteeInfo,
        role: Role,
        operator_pub_keys: &'a HashMap<OperatorId, Rsa<Public>>,
        slots_late: u64,
        fork_schedule: Arc<ForkSchedule>,
    ) -> ValidationContext<'a, ManualSlotClock> {
        let now = SystemTime::now();
        let slot_clock = ManualSlotClock::new(
            Slot::new(0),
            now.duration_since(UNIX_EPOCH).unwrap(),
            Duration::from_secs(12),
        );
        slot_clock.advance_slot();

        ValidationContext {
            signed_ssv_message: signed_msg,
            committee_info,
            role,
            received_at: now + Duration::from_secs(12 * slots_late),
            slots_per_epoch: SLOTS_PER_EPOCH_TEST,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock,
            operator_pub_keys,
            fork_schedule,
            spec: spec_with_gloas(None),
        }
    }

    // Helper to create a signed partial signature message for TTL tests
    fn create_signed_partial_sig_message(
        role: Role,
        kind: PartialSignatureKind,
        signer_id: OperatorId,
        private_key: &Rsa<Private>,
    ) -> SignedSSVMessage {
        let (mut partial_sig_messages, _) = create_test_partial_signature(
            role,
            kind,
            signer_id,
            PartialSigTestOptions::default(),
            Some(private_key.clone()),
        );
        partial_sig_messages.slot = Slot::new(1);

        let msg_id = create_message_id_for_test(role);
        let ssv_msg = SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            msg_id,
            partial_sig_messages.as_ssz_bytes(),
        )
        .unwrap();

        let p_key = PKey::from_rsa(private_key.clone()).unwrap();
        let mut signer = Signer::new(MessageDigest::sha256(), &p_key).unwrap();
        signer.update(&ssv_msg.as_ssz_bytes()).unwrap();
        let signature = signer.sign_to_vec().unwrap().try_into().unwrap();

        SignedSSVMessage::new(vec![signature], vec![signer_id], ssv_msg, vec![]).unwrap()
    }

    #[test]
    fn test_validator_registration_within_ttl_accepted() {
        // Setup
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signed_msg = create_signed_partial_sig_message(
            Role::ValidatorRegistration,
            PartialSignatureKind::ValidatorRegistration,
            OperatorId(1),
            &private_key,
        );

        let fork_schedule = ForkSchedule::new(Fork::Alan, DomainType::default(), "testing");
        let validation_context = create_ttl_validation_context(
            &signed_msg,
            &committee_info,
            Role::ValidatorRegistration,
            &map,
            TTL_SLOTS,
            Arc::new(fork_schedule),
        );

        // Execute
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        // Assert
        assert!(result.is_ok(), "Expected ok but got: {result:?}");
    }

    #[test]
    fn test_validator_registration_beyond_ttl_rejected() {
        // Setup
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signed_msg = create_signed_partial_sig_message(
            Role::ValidatorRegistration,
            PartialSignatureKind::ValidatorRegistration,
            OperatorId(1),
            &private_key,
        );

        let fork_schedule = ForkSchedule::new(Fork::Alan, DomainType::default(), "testing");
        let validation_context = create_ttl_validation_context(
            &signed_msg,
            &committee_info,
            Role::ValidatorRegistration,
            &map,
            BEYOND_TTL_SLOTS,
            Arc::new(fork_schedule),
        );

        // Execute
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        // Assert
        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::LateSlotMessage { .. }),
            "LateSlotMessage",
        );
    }

    #[test]
    fn test_voluntary_exit_within_ttl_accepted() {
        // Setup
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signed_msg = create_signed_partial_sig_message(
            Role::VoluntaryExit,
            PartialSignatureKind::VoluntaryExit,
            OperatorId(1),
            &private_key,
        );

        let fork_schedule = ForkSchedule::new(Fork::Alan, DomainType::default(), "testing");
        let validation_context = create_ttl_validation_context(
            &signed_msg,
            &committee_info,
            Role::VoluntaryExit,
            &map,
            TTL_SLOTS,
            Arc::new(fork_schedule),
        );

        // Execute
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 1,
            }),
        );

        // Assert
        assert!(result.is_ok(), "Expected ok but got: {result:?}");
    }

    #[test]
    fn test_aggregator_committee_skips_slot_advancement_check() {
        // Test that AggregatorCommittee role skips the slot advancement check
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let signer_id = OperatorId(1);

        // Create partial signature for slot 1
        let (mut partial_sig_messages, _) = create_test_partial_signature(
            Role::AggregatorCommittee,
            PartialSignatureKind::AggregatorCommitteePartialSig,
            signer_id,
            PartialSigTestOptions::default(),
            Some(private_key.clone()),
        );
        partial_sig_messages.slot = Slot::new(1);

        let msg_id = create_message_id_for_test(Role::AggregatorCommittee);
        let ssv_msg = SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            msg_id,
            partial_sig_messages.as_ssz_bytes(),
        )
        .unwrap();

        let p_key = PKey::from_rsa(private_key).unwrap();
        let mut signer = Signer::new(MessageDigest::sha256(), &p_key).unwrap();
        signer.update(&ssv_msg.as_ssz_bytes()).unwrap();
        let signature = signer.sign_to_vec().unwrap().try_into().unwrap();

        let signed_msg =
            SignedSSVMessage::new(vec![signature], vec![signer_id], ssv_msg, vec![]).unwrap();

        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);

        // Create a validation context where slot 1 has started
        let now = SystemTime::now();
        let slot_clock = ManualSlotClock::new(
            Slot::new(1),                            // Current slot is 1
            now.duration_since(UNIX_EPOCH).unwrap(), // Slot 1 starts now
            Duration::from_secs(12),
        );

        // Create fork schedule with Boole at epoch 0 (active from start)
        let fork_schedule = generate_fork_schedule(Fork::Boole);

        let validation_context = ValidationContext {
            signed_ssv_message: &signed_msg,
            committee_info: &committee_info,
            role: Role::AggregatorCommittee,
            received_at: now,
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock,
            operator_pub_keys: &map,
            fork_schedule,
            spec: spec_with_gloas(None),
        };

        // Create a duty state where the operator has already advanced to slot 10
        let mut duty_state = DutyState::new(64);
        // Process a dummy message for slot 10 to advance the operator's max_slot
        let dummy_messages = PartialSignatureMessages {
            kind: PartialSignatureKind::AggregatorCommitteePartialSig,
            slot: Slot::new(10),
            messages: VariableList::new(vec![PartialSignatureMessage {
                partial_signature: Signature::empty(),
                signing_root: Hash256::from([0u8; 32]),
                signer: signer_id,
                validator_index: ValidatorIndex(0),
            }])
            .unwrap(),
        };
        duty_state
            .update_for_partial_signature(&dummy_messages, &signer_id, 32)
            .unwrap();

        // Now validate a message for slot 1 (which is "old")
        let result = validate_partial_signature_message(
            validation_context,
            &mut duty_state,
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        // Should succeed because AggregatorCommittee skips the slot advancement check
        assert!(
            result.is_ok(),
            "Expected AggregatorCommittee to skip slot advancement check, but got: {:?}",
            result
        );
    }

    #[test]
    fn test_aggregator_committee_validator_index_occurrence_limit() {
        // Test that AggregatorCommittee allows up to 5 occurrences of the same validator index
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();

        // Create messages with the same validator index repeated 5 times (should pass)
        let messages: Vec<_> = (0..5)
            .map(|_| PartialSignatureMessage {
                partial_signature: Signature::empty(),
                signing_root: Hash256::from([0u8; 32]),
                signer: OperatorId(1),
                validator_index: ValidatorIndex(100), // Same validator index
            })
            .collect();

        let partial_sig_messages = PartialSignatureMessages {
            kind: PartialSignatureKind::AggregatorCommitteePartialSig,
            slot: Slot::new(1),
            messages: VariableList::new(messages).unwrap(),
        };

        let msg_id = create_message_id_for_test(Role::AggregatorCommittee);
        let ssv_msg = SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            msg_id,
            partial_sig_messages.as_ssz_bytes(),
        )
        .unwrap();

        let p_key = PKey::from_rsa(private_key.clone()).unwrap();
        let mut signer = Signer::new(MessageDigest::sha256(), &p_key).unwrap();
        signer.update(&ssv_msg.as_ssz_bytes()).unwrap();
        let signature = signer.sign_to_vec().unwrap().try_into().unwrap();

        let signed_msg =
            SignedSSVMessage::new(vec![signature], vec![OperatorId(1)], ssv_msg, vec![]).unwrap();

        let map = create_operator_pub_keys(
            committee_info.committee_members.clone(),
            vec![public_key.clone()],
        );

        // Create a validation context where slot 1 has started
        let now = SystemTime::now();
        let slot_clock = ManualSlotClock::new(
            Slot::new(1),                            // Current slot is 1
            now.duration_since(UNIX_EPOCH).unwrap(), // Slot 1 starts now
            Duration::from_secs(12),
        );

        // Create fork schedule with Boole at epoch 0 (active from start)
        let fork_schedule = generate_fork_schedule(Fork::Boole);

        let validation_context = ValidationContext {
            signed_ssv_message: &signed_msg,
            committee_info: &committee_info,
            role: Role::AggregatorCommittee,
            received_at: now,
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock,
            operator_pub_keys: &map,
            fork_schedule: fork_schedule.clone(),
            spec: spec_with_gloas(None),
        };

        // Should succeed with 5 occurrences
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert!(
            result.is_ok(),
            "Expected 5 occurrences of same validator index to be valid for AggregatorCommittee, but got: {:?}",
            result
        );

        // Now test with 6 occurrences (should fail)
        let messages: Vec<_> = (0..6)
            .map(|_| PartialSignatureMessage {
                partial_signature: Signature::empty(),
                signing_root: Hash256::from([0u8; 32]),
                signer: OperatorId(1),
                validator_index: ValidatorIndex(100), // Same validator index
            })
            .collect();

        let partial_sig_messages = PartialSignatureMessages {
            kind: PartialSignatureKind::AggregatorCommitteePartialSig,
            slot: Slot::new(1),
            messages: VariableList::new(messages).unwrap(),
        };

        let msg_id = create_message_id_for_test(Role::AggregatorCommittee);
        let ssv_msg = SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            msg_id,
            partial_sig_messages.as_ssz_bytes(),
        )
        .unwrap();

        let p_key = PKey::from_rsa(private_key).unwrap();
        let mut signer = Signer::new(MessageDigest::sha256(), &p_key).unwrap();
        signer.update(&ssv_msg.as_ssz_bytes()).unwrap();
        let signature = signer.sign_to_vec().unwrap().try_into().unwrap();

        let signed_msg =
            SignedSSVMessage::new(vec![signature], vec![OperatorId(1)], ssv_msg, vec![]).unwrap();

        // Reuse the same slot clock and fork schedule from the first part
        let slot_clock2 = ManualSlotClock::new(
            Slot::new(1),                            // Current slot is 1
            now.duration_since(UNIX_EPOCH).unwrap(), // Slot 1 starts now
            Duration::from_secs(12),
        );

        let validation_context = ValidationContext {
            signed_ssv_message: &signed_msg,
            committee_info: &committee_info,
            role: Role::AggregatorCommittee,
            received_at: now,
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock: slot_clock2,
            operator_pub_keys: &map,
            fork_schedule,
            spec: spec_with_gloas(None),
        };

        // Should fail with 6 occurrences
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(2),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::TooManyValidatorIndexOccurrences { limit: 5, .. }
                )
            },
            "TooManyValidatorIndexOccurrences (limit exceeded for AggregatorCommittee)",
        );
    }

    #[test]
    fn test_ptc_attester_rejects_multiple_messages_per_packet() {
        // PTC is validator-scoped: exactly one PayloadAttestationMessage per
        // packet. The per-validator `> 1` bound rejects a second message, which
        // also subsumes the validator-index occurrence cap (two messages for the
        // same index would already trip `> 1`).
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);

        // ValidatorIndex(123) is in the test committee's validator_indices, so
        // each message passes the per-validator index check; the second message
        // then trips the `> 1` packet bound.
        let messages: Vec<_> = (0..2)
            .map(|_| PartialSignatureMessage {
                partial_signature: Signature::empty(),
                signing_root: Hash256::from([0u8; 32]),
                signer: OperatorId(1),
                validator_index: ValidatorIndex(123),
            })
            .collect();

        let partial_sig_messages = PartialSignatureMessages {
            kind: PartialSignatureKind::PTCAttester,
            slot: Slot::new(1),
            messages: VariableList::new(messages).unwrap(),
        };

        let msg_id = create_message_id_for_test(Role::PTCAttester);
        let ssv_msg = SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            msg_id,
            partial_sig_messages.as_ssz_bytes(),
        )
        .unwrap();

        let p_key = PKey::from_rsa(private_key).unwrap();
        let mut signer = Signer::new(MessageDigest::sha256(), &p_key).unwrap();
        signer.update(&ssv_msg.as_ssz_bytes()).unwrap();
        let signature = signer.sign_to_vec().unwrap().try_into().unwrap();

        let signed_msg =
            SignedSSVMessage::new(vec![signature], vec![OperatorId(1)], ssv_msg, vec![]).unwrap();

        let now = SystemTime::now();
        let slot_clock = ManualSlotClock::new(
            Slot::new(1),
            now.duration_since(UNIX_EPOCH).unwrap(),
            Duration::from_secs(12),
        );

        let validation_context = ValidationContext {
            signed_ssv_message: &signed_msg,
            committee_info: &committee_info,
            role: Role::PTCAttester,
            received_at: now,
            slots_per_epoch: 32,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock,
            operator_pub_keys: &map,
            fork_schedule: generate_fork_schedule(Fork::Boole),
            spec: spec_with_gloas(Some(0)),
        };

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::TooManyPartialSignatureMessages { limit: 1, .. }
                )
            },
            "TooManyPartialSignatureMessages (PTC cap 1 per packet)",
        );
    }

    #[test]
    fn test_ptc_attester_rejected_before_gloas() {
        use crate::validate_role_for_fork;

        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signed_msg = create_signed_partial_sig_message(
            Role::PTCAttester,
            PartialSignatureKind::PTCAttester,
            OperatorId(1),
            &private_key,
        );

        let fork_schedule = generate_fork_schedule(Fork::Boole);
        let validation_context = create_test_validation_context_with_fork(
            &signed_msg,
            &committee_info,
            Role::PTCAttester,
            &map,
            Some(fork_schedule),
        );

        let result = validate_role_for_fork(Slot::new(0), &validation_context);
        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::RoleNotActiveBeforeEthFork {
                        minimum_fork: types::ForkName::Gloas,
                        ..
                    }
                )
            },
            "RoleNotActiveBeforeEthFork (PTCAttester pre-Gloas)",
        );
    }

    #[test]
    fn test_ptc_attester_within_ttl_accepted() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signed_msg = create_signed_partial_sig_message(
            Role::PTCAttester,
            PartialSignatureKind::PTCAttester,
            OperatorId(1),
            &private_key,
        );

        // PTC output is only useful for its own slot (the aggregated payload
        // attestation is gossip-valid for that slot and includable only at
        // slot + 1), so PTCAttester uses the short TTL
        // (1 + LATE_SLOT_ALLOWANCE = 3 slots). Two slots late is inside it.
        let mut validation_context = create_ttl_validation_context(
            &signed_msg,
            &committee_info,
            Role::PTCAttester,
            &map,
            LATE_SLOT_ALLOWANCE_TEST,
            generate_fork_schedule(Fork::Boole),
        );
        // PTCAttester only exists post-Gloas; the role gate reads the Ethereum fork from the spec.
        validation_context.spec = spec_with_gloas(Some(0));

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert!(result.is_ok(), "Expected ok but got: {result:?}");
    }

    #[test]
    fn test_ptc_attester_beyond_short_ttl_rejected() {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signed_msg = create_signed_partial_sig_message(
            Role::PTCAttester,
            PartialSignatureKind::PTCAttester,
            OperatorId(1),
            &private_key,
        );

        // 20 slots late would still be inside the long (committee) TTL of 34
        // slots; rejecting it pins PTCAttester to the short slot-bound bucket.
        let mut validation_context = create_ttl_validation_context(
            &signed_msg,
            &committee_info,
            Role::PTCAttester,
            &map,
            COMMITTEE_TTL_BUCKET_SLOTS,
            generate_fork_schedule(Fork::Boole),
        );
        // PTCAttester only exists post-Gloas; the role gate reads the Ethereum fork from the spec.
        validation_context.spec = spec_with_gloas(Some(0));

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::LateSlotMessage { .. }),
            "LateSlotMessage (PTCAttester past short TTL)",
        );
    }

    #[test]
    fn test_ptc_attester_binds_ptc_attester_kind() {
        // PTC binds to its own PartialSignatureKind::PTCAttester (no longer
        // PostConsensus). Mapping to ValidationFailure::PartialSignatureTypeRoleMismatch
        // is covered end-to-end by test_partial_signature_message_with_invalid_type_for_role.
        assert!(partial_signature_type_matches_role(
            PartialSignatureKind::PTCAttester,
            Role::PTCAttester,
        ));
        assert!(!partial_signature_type_matches_role(
            PartialSignatureKind::PostConsensus,
            Role::PTCAttester,
        ));
    }

    // ==================== ProposerPreferences tests ====================
    //
    // ProposerPreferences (wire byte [8,0,0,0]) is validator-scoped and non-QBFT.
    // It mirrors PTCAttester's fork gate (post-Gloas only) and per-packet cap (1),
    // but sits in the LONG TTL bucket (like ValidatorRegistration) and carries a
    // novel per-slot signing-root dedup: a proposer legitimately signs multiple
    // distinct preference roots at one send slot (the current + next-epoch batch).

    /// Epoch at which the Ethereum Gloas fork activates in the ProposerPreferences
    /// fork-gate tests. Message slots below `GLOAS_ACTIVATION_EPOCH * SLOTS_PER_EPOCH_TEST`
    /// are pre-Gloas and must be rejected.
    const GLOAS_ACTIVATION_EPOCH: u64 = 1;

    /// Builds a signed ProposerPreferences partial-signature packet with a caller-chosen
    /// `signing_root` and envelope `slot`. The envelope slot is the CURRENT SEND SLOT;
    /// the future `proposal_slot` a real body would carry is never on the wire, so only
    /// the send-slot governs timing and per-slot dedup. Signed with `private_key` so it
    /// passes signature verification end-to-end.
    fn create_signed_proposer_preferences_message(
        signer_id: OperatorId,
        private_key: &Rsa<Private>,
        slot: Slot,
        signing_root: Hash256,
    ) -> SignedSSVMessage {
        let partial_sig_messages = PartialSignatureMessages {
            kind: PartialSignatureKind::ProposerPreferences,
            slot,
            messages: VariableList::new(vec![PartialSignatureMessage {
                partial_signature: Signature::empty(),
                signing_root,
                signer: signer_id,
                // ValidatorIndex(0) is in the test committee's validator_indices.
                validator_index: ValidatorIndex(0),
            }])
            .unwrap(),
        };

        let msg_id = create_message_id_for_test(Role::ProposerPreferences);
        let ssv_msg = SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            msg_id,
            partial_sig_messages.as_ssz_bytes(),
        )
        .unwrap();

        let p_key = PKey::from_rsa(private_key.clone()).unwrap();
        let mut signer = Signer::new(MessageDigest::sha256(), &p_key).unwrap();
        signer.update(&ssv_msg.as_ssz_bytes()).unwrap();
        let signature = signer.sign_to_vec().unwrap().try_into().unwrap();

        SignedSSVMessage::new(vec![signature], vec![signer_id], ssv_msg, vec![]).unwrap()
    }

    /// Builds a ProposerPreferences validation context whose send slot matches
    /// the packet's envelope slot, so `validate_slot_time` sees an on-time message.
    /// The Ethereum Gloas fork is active from epoch 0 unless overridden by the caller.
    fn create_proposer_preferences_context<'a>(
        signed_msg: &'a SignedSSVMessage,
        committee_info: &'a crate::CommitteeInfo,
        operator_pub_keys: &'a HashMap<OperatorId, Rsa<Public>>,
        send_slot: Slot,
    ) -> ValidationContext<'a, ManualSlotClock> {
        let now = SystemTime::now();
        let slot_clock = ManualSlotClock::new(
            send_slot,
            now.duration_since(UNIX_EPOCH).unwrap(),
            Duration::from_secs(12),
        );

        ValidationContext {
            signed_ssv_message: signed_msg,
            committee_info,
            role: Role::ProposerPreferences,
            received_at: now,
            slots_per_epoch: SLOTS_PER_EPOCH_TEST,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock,
            operator_pub_keys,
            fork_schedule: generate_fork_schedule(Fork::Boole),
            // ProposerPreferences only exists post-Gloas; activate from epoch 0.
            spec: spec_with_gloas(Some(0)),
        }
    }

    #[test]
    fn test_proposer_preferences_rejected_before_gloas() {
        use crate::validate_role_for_fork;

        // Arrange: a ProposerPreferences packet plus a spec whose Gloas fork
        // activates at GLOAS_ACTIVATION_EPOCH. Slots before that epoch are pre-Gloas.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signed_msg = create_signed_partial_sig_message(
            Role::ProposerPreferences,
            PartialSignatureKind::ProposerPreferences,
            OperatorId(1),
            &private_key,
        );

        let mut validation_context = create_test_validation_context_with_fork(
            &signed_msg,
            &committee_info,
            Role::ProposerPreferences,
            &map,
            Some(generate_fork_schedule(Fork::Boole)),
        );
        validation_context.spec = spec_with_gloas(Some(GLOAS_ACTIVATION_EPOCH));

        // Act + Assert: slot 0 (epoch 0) is before Gloas activation → rejected.
        let pre_gloas_slot = Slot::new(0);
        let result = validate_role_for_fork(pre_gloas_slot, &validation_context);
        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::RoleNotActiveBeforeEthFork {
                        minimum_fork: types::ForkName::Gloas,
                        ..
                    }
                )
            },
            "RoleNotActiveBeforeEthFork (ProposerPreferences pre-Gloas)",
        );

        // Boundary: the last slot of the epoch immediately before activation is
        // still pre-Gloas → rejected.
        let boundary_slot = Slot::new(GLOAS_ACTIVATION_EPOCH * SLOTS_PER_EPOCH_TEST - 1);
        let result = validate_role_for_fork(boundary_slot, &validation_context);
        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::RoleNotActiveBeforeEthFork {
                        minimum_fork: types::ForkName::Gloas,
                        ..
                    }
                )
            },
            "RoleNotActiveBeforeEthFork (ProposerPreferences at activation boundary)",
        );

        // Post-Gloas: the first slot of the activation epoch passes the fork gate.
        let post_gloas_slot = Slot::new(GLOAS_ACTIVATION_EPOCH * SLOTS_PER_EPOCH_TEST);
        let result = validate_role_for_fork(post_gloas_slot, &validation_context);
        assert!(
            result.is_ok(),
            "Expected ProposerPreferences to pass the fork gate at Gloas activation, got: {result:?}"
        );

        // A spec where Gloas never activates rejects at every slot.
        validation_context.spec = spec_with_gloas(None);
        let result = validate_role_for_fork(post_gloas_slot, &validation_context);
        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::RoleNotActiveBeforeEthFork {
                        minimum_fork: types::ForkName::Gloas,
                        ..
                    }
                )
            },
            "RoleNotActiveBeforeEthFork (ProposerPreferences, Gloas never activates)",
        );
    }

    #[test]
    fn test_proposer_preferences_accepted_at_gloas() {
        // Arrange: at/after Gloas activation the role passes the full pipeline.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signed_msg = create_signed_partial_sig_message(
            Role::ProposerPreferences,
            PartialSignatureKind::ProposerPreferences,
            OperatorId(1),
            &private_key,
        );

        // The signed packet's envelope slot is 1; send from slot 1 so timing is on-time.
        let validation_context =
            create_proposer_preferences_context(&signed_msg, &committee_info, &map, Slot::new(1));

        // Act
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        // Assert
        assert!(result.is_ok(), "Expected ok but got: {result:?}");
    }

    #[test]
    fn test_proposer_preferences_invalid_partial_sig_kind_rejected() {
        // Arrange: a packet declaring Role::ProposerPreferences but carrying a
        // non-ProposerPreferences kind must be rejected by the role/kind binding.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signed_msg = create_signed_partial_sig_message(
            Role::ProposerPreferences,
            PartialSignatureKind::PostConsensus, // Invalid for ProposerPreferences
            OperatorId(1),
            &private_key,
        );

        let validation_context =
            create_proposer_preferences_context(&signed_msg, &committee_info, &map, Slot::new(1));

        // Act
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        // Assert
        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::PartialSignatureTypeRoleMismatch),
            "PartialSignatureTypeRoleMismatch (ProposerPreferences kind mismatch)",
        );
    }

    #[test]
    fn test_proposer_preferences_within_ttl_accepted() {
        // Arrange: ProposerPreferences sits in the LONG (committee-style) TTL bucket
        // of slots_per_epoch + LATE_SLOT_ALLOWANCE = 34 slots, unlike PTCAttester's
        // short 3-slot window. A message exactly TTL_SLOTS late is still inside it.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signed_msg = create_signed_partial_sig_message(
            Role::ProposerPreferences,
            PartialSignatureKind::ProposerPreferences,
            OperatorId(1),
            &private_key,
        );

        let mut validation_context = create_ttl_validation_context(
            &signed_msg,
            &committee_info,
            Role::ProposerPreferences,
            &map,
            TTL_SLOTS,
            generate_fork_schedule(Fork::Boole),
        );
        // ProposerPreferences only exists post-Gloas; the role gate reads the Ethereum fork.
        validation_context.spec = spec_with_gloas(Some(0));

        // Act
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        // Assert
        assert!(result.is_ok(), "Expected ok but got: {result:?}");
    }

    #[test]
    fn test_proposer_preferences_beyond_ttl_rejected() {
        // Arrange: a message BEYOND_TTL_SLOTS (40) late is past the long 34-slot
        // window, so it is rejected as late. This pins ProposerPreferences to the
        // long bucket rather than the short slot-bound one.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signed_msg = create_signed_partial_sig_message(
            Role::ProposerPreferences,
            PartialSignatureKind::ProposerPreferences,
            OperatorId(1),
            &private_key,
        );

        let mut validation_context = create_ttl_validation_context(
            &signed_msg,
            &committee_info,
            Role::ProposerPreferences,
            &map,
            BEYOND_TTL_SLOTS,
            generate_fork_schedule(Fork::Boole),
        );
        validation_context.spec = spec_with_gloas(Some(0));

        // Act
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        // Assert
        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::LateSlotMessage { .. }),
            "LateSlotMessage (ProposerPreferences past long TTL)",
        );
    }

    #[test]
    fn test_proposer_preferences_message_count_at_most_one() {
        // Arrange: ProposerPreferences is per-validator; a packet with two
        // PartialSignatureMessages trips the `> 1` per-packet bound.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);

        let messages: Vec<_> = (0..2)
            .map(|_| PartialSignatureMessage {
                partial_signature: Signature::empty(),
                signing_root: Hash256::from([0u8; 32]),
                signer: OperatorId(1),
                validator_index: ValidatorIndex(0),
            })
            .collect();

        let partial_sig_messages = PartialSignatureMessages {
            kind: PartialSignatureKind::ProposerPreferences,
            slot: Slot::new(1),
            messages: VariableList::new(messages).unwrap(),
        };

        let msg_id = create_message_id_for_test(Role::ProposerPreferences);
        let ssv_msg = SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            msg_id,
            partial_sig_messages.as_ssz_bytes(),
        )
        .unwrap();

        let p_key = PKey::from_rsa(private_key).unwrap();
        let mut signer = Signer::new(MessageDigest::sha256(), &p_key).unwrap();
        signer.update(&ssv_msg.as_ssz_bytes()).unwrap();
        let signature = signer.sign_to_vec().unwrap().try_into().unwrap();

        let signed_msg =
            SignedSSVMessage::new(vec![signature], vec![OperatorId(1)], ssv_msg, vec![]).unwrap();

        let validation_context =
            create_proposer_preferences_context(&signed_msg, &committee_info, &map, Slot::new(1));

        // Act
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        // Assert
        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::TooManyPartialSignatureMessages { limit: 1, .. }
                )
            },
            "TooManyPartialSignatureMessages (ProposerPreferences cap 1 per packet)",
        );
    }

    #[test]
    fn test_proposer_preferences_future_proposal_slot_in_body_not_early() {
        // A ProposerPreferences body describes a FUTURE proposal_slot, but that
        // field is not on the wire: only the envelope (send) slot is. As long as
        // the envelope slot equals the current send slot, validate_slot_time must
        // accept it (no EarlySlotMessage), even though the intended proposal is
        // for a later slot.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);

        // Envelope slot is the current send slot (100); a real body would carry a
        // far-future proposal_slot, but that never reaches the validator.
        let send_slot = Slot::new(100);
        let signed_msg = create_signed_proposer_preferences_message(
            OperatorId(1),
            &private_key,
            send_slot,
            Hash256::from([1u8; 32]),
        );

        let validation_context =
            create_proposer_preferences_context(&signed_msg, &committee_info, &map, send_slot);

        // Act
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(256),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        // Assert: accepted; specifically NOT rejected as early.
        assert!(
            result.is_ok(),
            "Expected on-time ProposerPreferences (envelope==send slot) to be accepted, got: {result:?}"
        );
    }

    #[test]
    fn test_proposer_preferences_distinct_roots_same_send_slot_accepted() {
        // A proposer legitimately signs multiple distinct preference roots at one
        // send slot (current + next-epoch lookahead batch). Two packets from the
        // same (validator, operator, envelope slot) with DIFFERENT signing roots
        // must BOTH be accepted through a shared DutyState.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signer_id = OperatorId(1);
        let send_slot = Slot::new(1);
        let mut duty_state = DutyState::new(64);

        // First packet: root A.
        let signed_a = create_signed_proposer_preferences_message(
            signer_id,
            &private_key,
            send_slot,
            Hash256::from([0xAA; 32]),
        );
        let context_a =
            create_proposer_preferences_context(&signed_a, &committee_info, &map, send_slot);
        let result_a = validate_partial_signature_message(
            context_a,
            &mut duty_state,
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );
        assert!(
            result_a.is_ok(),
            "Expected first distinct-root packet to be accepted, got: {result_a:?}"
        );

        // Second packet: DIFFERENT root B at the SAME send slot.
        let signed_b = create_signed_proposer_preferences_message(
            signer_id,
            &private_key,
            send_slot,
            Hash256::from([0xBB; 32]),
        );
        let context_b =
            create_proposer_preferences_context(&signed_b, &committee_info, &map, send_slot);
        let result_b = validate_partial_signature_message(
            context_b,
            &mut duty_state,
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        // Assert: the second distinct root is also accepted (NOT falsely rejected
        // as a pre-consensus duplicate).
        assert!(
            result_b.is_ok(),
            "Expected second distinct-root packet at same send slot to be accepted, got: {result_b:?}"
        );
    }

    #[test]
    fn test_proposer_preferences_duplicate_root_same_slot_rejected() {
        // An exact-duplicate signing_root at the same send slot is a resend and
        // must be rejected as a duplicate, via the per-slot seen_preferences set.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signer_id = OperatorId(1);
        let send_slot = Slot::new(1);
        let root = Hash256::from([0xCC; 32]);
        let mut duty_state = DutyState::new(64);

        // First packet with root R is accepted.
        let signed_first =
            create_signed_proposer_preferences_message(signer_id, &private_key, send_slot, root);
        let context_first =
            create_proposer_preferences_context(&signed_first, &committee_info, &map, send_slot);
        let result_first = validate_partial_signature_message(
            context_first,
            &mut duty_state,
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );
        assert!(
            result_first.is_ok(),
            "Expected first packet to be accepted, got: {result_first:?}"
        );

        // Second packet: EXACT-DUPLICATE root R at the SAME send slot.
        let signed_dup =
            create_signed_proposer_preferences_message(signer_id, &private_key, send_slot, root);
        let context_dup =
            create_proposer_preferences_context(&signed_dup, &committee_info, &map, send_slot);
        let result_dup = validate_partial_signature_message(
            context_dup,
            &mut duty_state,
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        // Assert
        assert_validation_error(
            result_dup,
            |failure| matches!(failure, ValidationFailure::DuplicatedMessage { .. }),
            "DuplicatedMessage (ProposerPreferences exact-duplicate root)",
        );
    }

    #[test]
    fn test_proposer_preferences_root_cap_enforced() {
        // The distinct-root set for one (validator, operator, send slot) is capped
        // at 2 * slots_per_epoch. Feeding exactly that many distinct roots all pass;
        // one more distinct root is rejected as an invalid type count.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signer_id = OperatorId(1);
        let send_slot = Slot::new(1);
        let mut duty_state = DutyState::new(64);

        // Use the harness slots_per_epoch so the loop stays small (2 * 32 = 64).
        let cap = 2 * SLOTS_PER_EPOCH_TEST;

        // Feed `cap` distinct roots; all must be accepted.
        for i in 0..cap {
            let mut root_bytes = [0u8; 32];
            root_bytes[0..8].copy_from_slice(&i.to_le_bytes());
            let signed = create_signed_proposer_preferences_message(
                signer_id,
                &private_key,
                send_slot,
                Hash256::from(root_bytes),
            );
            let context =
                create_proposer_preferences_context(&signed, &committee_info, &map, send_slot);
            let result = validate_partial_signature_message(
                context,
                &mut duty_state,
                Arc::new(MockDutiesProvider {
                    voluntary_exit_duty_count: 0,
                }),
            );
            assert!(
                result.is_ok(),
                "Expected distinct root #{i} (within cap {cap}) to be accepted, got: {result:?}"
            );
        }

        // One more distinct root exceeds the cap and is rejected.
        let mut over_cap_bytes = [0u8; 32];
        over_cap_bytes[0..8].copy_from_slice(&cap.to_le_bytes());
        let signed_over = create_signed_proposer_preferences_message(
            signer_id,
            &private_key,
            send_slot,
            Hash256::from(over_cap_bytes),
        );
        let context_over =
            create_proposer_preferences_context(&signed_over, &committee_info, &map, send_slot);
        let result_over = validate_partial_signature_message(
            context_over,
            &mut duty_state,
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        // Assert
        assert_validation_error(
            result_over,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::InvalidPartialSignatureTypeCount { .. }
                )
            },
            "InvalidPartialSignatureTypeCount (ProposerPreferences distinct-root cap)",
        );
    }

    #[test]
    fn test_voluntary_exit_beyond_ttl_rejected() {
        // Setup
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signed_msg = create_signed_partial_sig_message(
            Role::VoluntaryExit,
            PartialSignatureKind::VoluntaryExit,
            OperatorId(1),
            &private_key,
        );

        let fork_schedule = ForkSchedule::new(Fork::Alan, DomainType::default(), "testing");
        let validation_context = create_ttl_validation_context(
            &signed_msg,
            &committee_info,
            Role::VoluntaryExit,
            &map,
            BEYOND_TTL_SLOTS,
            Arc::new(fork_schedule),
        );

        // Execute
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 1,
            }),
        );

        // Assert
        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::LateSlotMessage { .. }),
            "LateSlotMessage",
        );
    }

    #[test]
    fn test_aggregator_partial_sig_rejected_after_boole() {
        // Arrange
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);

        let (_, signed_msg) = create_test_partial_signature(
            Role::Aggregator,
            PartialSignatureKind::SelectionProofPartialSig,
            OperatorId(1),
            PartialSigTestOptions::default(),
            Some(private_key),
        );

        // Create validation context with Boole fork (Boole is active)
        let validation_context = create_test_validation_context_with_fork(
            &signed_msg,
            &committee_info,
            Role::Aggregator,
            &map,
            Some(generate_fork_schedule(Fork::Boole)),
        );

        // Act
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
            }),
        );

        // Assert - Should be rejected after Boole
        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::RoleNotActiveAfterFork {
                        role: Role::Aggregator,
                        deprecated_since_fork: Fork::Boole,
                        ..
                    }
                )
            },
            "RoleNotActiveAfterFork for Aggregator",
        );
    }
}

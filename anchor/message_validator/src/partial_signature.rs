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
        Role::EnvelopeProposer => kind == PartialSignatureKind::PostConsensus,
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

    // Rule: Slot must not be "old" - a monotonic-slot signer must not have already advanced to a
    // later slot. Committee roles (batched, slot-keyed) and ProposerPreferences (holds its whole
    // proposal-slot lookahead concurrently, so a lower slot is a concurrent duty, not a stale one)
    // are exempt; their replay bound is the earliness/lateness window instead.
    if role.monotonic_slot_role() {
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
        | Role::ProposerPreferences
        | Role::EnvelopeProposer => {
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
    use crate::{
        MessageAcceptance,
        tests::{
            FOUR_NODE_COMMITTEE, MockDutiesProvider, assert_validation_error,
            create_committee_info, create_message_id_for_test, create_operator_pub_keys,
            generate_random_rsa_public_keys, spec_with_gloas,
        },
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
    fn test_validator_registration_rejected_after_gloas() {
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
        let mut validation_context = create_ttl_validation_context(
            &signed_msg,
            &committee_info,
            Role::ValidatorRegistration,
            &map,
            TTL_SLOTS,
            Arc::new(fork_schedule),
        );
        // ValidatorRegistration is deprecated at Gloas; the role gate reads the Ethereum
        // fork from the spec. The message slot (1) is in epoch 0, where Gloas is active.
        validation_context.spec = spec_with_gloas(Some(0));

        // Execute
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
                ..Default::default()
            }),
        );

        // Assert
        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::RoleNotActiveAfterEthFork {
                        role: Role::ValidatorRegistration,
                        deprecated_since_fork: types::ForkName::Gloas,
                        ..
                    }
                )
            },
            "RoleNotActiveAfterEthFork (ValidatorRegistration post-Gloas)",
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
    fn test_validator_registration_rejected_at_gloas_boundary() {
        use crate::validate_role_for_fork;

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

        let fork_schedule = generate_fork_schedule(Fork::Boole);
        let mut validation_context = create_test_validation_context_with_fork(
            &signed_msg,
            &committee_info,
            Role::ValidatorRegistration,
            &map,
            Some(fork_schedule),
        );
        // The role gate reads the Ethereum fork from the spec; Gloas activates at epoch 2.
        validation_context.spec = spec_with_gloas(Some(2));

        // Slot 63 is the last slot of epoch 1 (32 slots per epoch): still pre-Gloas,
        // so registrations remain valid.
        let result = validate_role_for_fork(Slot::new(63), &validation_context);
        assert!(result.is_ok(), "Expected ok but got: {result:?}");

        // Slot 64 is the first slot of epoch 2: Gloas is active, the deprecated
        // duty is rejected.
        let result = validate_role_for_fork(Slot::new(64), &validation_context);
        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::RoleNotActiveAfterEthFork {
                        role: Role::ValidatorRegistration,
                        deprecated_since_fork: types::ForkName::Gloas,
                        ..
                    }
                )
            },
            "RoleNotActiveAfterEthFork (ValidatorRegistration post-Gloas)",
        );

        // Pins the mainnet no-op: with Gloas unscheduled, the deprecation gate
        // never fires regardless of slot.
        validation_context.spec = spec_with_gloas(None);
        let result = validate_role_for_fork(Slot::new(100_000), &validation_context);
        assert!(result.is_ok(), "Expected ok but got: {result:?}");
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
                ..Default::default()
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
                ..Default::default()
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

    // ==================== EnvelopeProposer partial-signature tests ====================
    //
    // `EnvelopeProposer` (wire byte [9,0,0,0]) is the validator-scoped, QBFT self-build role:
    // binds `PostConsensus`, gated to the Ethereum Gloas (ePBS) fork, caps its packet at one
    // message, and uses the SHORT `1 + LATE_SLOT_ALLOWANCE` (3-slot) lateness bucket.

    /// Standard single-signer four-node fixture (committee info + keypair + pubkey map).
    fn four_node_committee_and_keypair() -> (
        crate::CommitteeInfo,
        Rsa<Private>,
        HashMap<OperatorId, Rsa<Public>>,
    ) {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        (committee_info, private_key, map)
    }

    /// Helper to create a SignedSSVMessage for EnvelopeProposer testing.
    fn create_signed_envelope_proposer_message(
        signer_id: OperatorId,
        private_key: &Rsa<Private>,
        slot: Slot,
        signing_root: Hash256,
    ) -> SignedSSVMessage {
        let partial_sig_messages = PartialSignatureMessages {
            kind: PartialSignatureKind::PostConsensus,
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

        let msg_id = create_message_id_for_test(Role::EnvelopeProposer);
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

    /// Helper to create a ValidationContext for EnvelopeProposer testing.
    fn create_envelope_proposer_context<'a>(
        signed_msg: &'a SignedSSVMessage,
        committee_info: &'a crate::CommitteeInfo,
        operator_pub_keys: &'a HashMap<OperatorId, Rsa<Public>>,
        current_slot: Slot,
    ) -> ValidationContext<'a, ManualSlotClock> {
        let now = SystemTime::now();
        let slot_clock = ManualSlotClock::new(
            current_slot,
            now.duration_since(UNIX_EPOCH).unwrap(),
            Duration::from_secs(12),
        );

        ValidationContext {
            signed_ssv_message: signed_msg,
            committee_info,
            role: Role::EnvelopeProposer,
            received_at: now,
            slots_per_epoch: SLOTS_PER_EPOCH_TEST,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock,
            operator_pub_keys,
            fork_schedule: generate_fork_schedule(Fork::Boole),
            // EnvelopeProposer only exists post-Gloas; activate from epoch 0.
            spec: spec_with_gloas(Some(0)),
        }
    }

    /// `EnvelopeProposer` only accepts the `PostConsensus` kind (the Gloas fork gate is
    /// covered once in `consensus_message.rs` via the shared `validate_role_for_fork`).
    /// Asserts that this kind mismatch maps to
    /// `ValidationFailure::PartialSignatureTypeRoleMismatch` and is rejected (not ignored).
    #[test]
    fn envelope_proposer_binds_post_consensus_kind() {
        assert!(
            partial_signature_type_matches_role(
                PartialSignatureKind::PostConsensus,
                Role::EnvelopeProposer,
            ),
            "EnvelopeProposer must accept PostConsensus"
        );
        assert_eq!(
            MessageAcceptance::from(&ValidationFailure::PartialSignatureTypeRoleMismatch),
            MessageAcceptance::Reject,
            "kind mismatch must be Reject",
        );
        assert!(
            !partial_signature_type_matches_role(
                PartialSignatureKind::RandaoPartialSig,
                Role::EnvelopeProposer,
            ),
            "EnvelopeProposer must reject non-PostConsensus kinds"
        );
    }

    #[test]
    fn envelope_proposer_rejects_multiple_messages_per_packet() {
        // EnvelopeProposer is validator-scoped: exactly one PostConsensus message per packet.
        // The per-validator `> 1` bound rejects a second message, which also subsumes the
        // validator-index occurrence cap (two messages for the same index would already trip
        // `> 1`).
        let (committee_info, private_key, map) = four_node_committee_and_keypair();

        // ValidatorIndex(123) is in the test committee's validator_indices, so each message
        // passes the per-validator index check; the second message then trips the `> 1` bound.
        let messages: Vec<_> = (0..2)
            .map(|_| PartialSignatureMessage {
                partial_signature: Signature::empty(),
                signing_root: Hash256::from([0u8; 32]),
                signer: OperatorId(1),
                validator_index: ValidatorIndex(123),
            })
            .collect();

        let partial_sig_messages = PartialSignatureMessages {
            kind: PartialSignatureKind::PostConsensus,
            slot: Slot::new(1),
            messages: VariableList::new(messages).unwrap(),
        };

        let msg_id = create_message_id_for_test(Role::EnvelopeProposer);
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
            create_envelope_proposer_context(&signed_msg, &committee_info, &map, Slot::new(1));

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
                ..Default::default()
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
            "TooManyPartialSignatureMessages (EnvelopeProposer cap 1 per packet)",
        );

        // Check that more than 1 partial signature message in a packet is rejected and not ignored.
        assert_eq!(
            MessageAcceptance::from(&ValidationFailure::TooManyPartialSignatureMessages {
                got: 2,
                limit: 1
            }),
            MessageAcceptance::Reject,
            "packet-count overflow must be Reject",
        );
    }

    #[test]
    fn envelope_proposer_within_ttl_accepted() {
        let (committee_info, private_key, map) = four_node_committee_and_keypair();
        let signed_msg = create_signed_partial_sig_message(
            Role::EnvelopeProposer,
            PartialSignatureKind::PostConsensus,
            OperatorId(1),
            &private_key,
        );

        // `EnvelopeProposer` uses the SHORT slot-bound TTL (`1 + LATE_SLOT_ALLOWANCE` = 3 slots);
        // two slots late is inside it. `create_ttl_validation_context` sets a pre-Gloas spec, so
        // override it (the role only exists post-Gloas).
        let mut validation_context = create_ttl_validation_context(
            &signed_msg,
            &committee_info,
            Role::EnvelopeProposer,
            &map,
            LATE_SLOT_ALLOWANCE_TEST,
            generate_fork_schedule(Fork::Boole),
        );
        validation_context.spec = spec_with_gloas(Some(0));

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
                ..Default::default()
            }),
        );

        assert!(result.is_ok(), "Expected ok but got: {result:?}");
    }

    #[test]
    fn envelope_proposer_beyond_short_ttl_rejected() {
        let (committee_info, private_key, map) = four_node_committee_and_keypair();
        let signed_msg = create_signed_partial_sig_message(
            Role::EnvelopeProposer,
            PartialSignatureKind::PostConsensus,
            OperatorId(1),
            &private_key,
        );

        // 20 slots late is still inside the long (committee) TTL of 34 slots; rejecting it pins
        // `EnvelopeProposer` to the short slot-bound bucket (`1 + LATE_SLOT_ALLOWANCE` = 3 slots).
        // Override the helper's pre-Gloas spec (the role only exists post-Gloas).
        let mut validation_context = create_ttl_validation_context(
            &signed_msg,
            &committee_info,
            Role::EnvelopeProposer,
            &map,
            COMMITTEE_TTL_BUCKET_SLOTS,
            generate_fork_schedule(Fork::Boole),
        );
        validation_context.spec = spec_with_gloas(Some(0));

        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                voluntary_exit_duty_count: 0,
                ..Default::default()
            }),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::LateSlotMessage { .. }),
            "LateSlotMessage (EnvelopeProposer past short TTL)",
        );
    }

    // ==================== ProposerPreferences tests ====================
    //
    // ProposerPreferences (wire byte [8,0,0,0]) is validator-scoped and non-QBFT.
    // It mirrors PTCAttester's fork gate (post-Gloas only) and per-packet cap (1),
    // but its envelope slot is the duty's (possibly future) `proposal_slot`, not the
    // current send slot. That single semantic change drives six role-scoped rules:
    //   1. earliness: a future `proposal_slot` up to `(1 + spec.min_seed_lookahead) *
    //      slots_per_epoch` ahead is accepted (`early_slot_allowance`), while every other role
    //      keeps the strict no-future rule;
    //   2. lateness: a TIGHT TTL of `LATE_SLOT_ALLOWANCE` slots past `proposal_slot`;
    //   3. no monotonic slot-advance (a lower `proposal_slot` is a concurrent duty, not a stale
    //      one);
    //   4. proposer-assignment arm keyed on `proposal_slot` in `validate_beacon_duty`;
    //   5. a per-role `DutyState` ring sized to cover the lookahead so a future-slot write cannot
    //      evict a live slot's dedup state;
    //   6. per-`proposal_slot` signing-root dedup capped at
    //      `MAX_PROPOSER_PREFERENCES_DISTINCT_ROOTS` (over-cap → `TooManyDistinctSigningRoots` /
    //      Ignore, exact-dup → `DuplicatedMessage` / Reject).

    /// Epoch at which the Ethereum Gloas fork activates in the ProposerPreferences
    /// fork-gate tests. Message slots below `GLOAS_ACTIVATION_EPOCH * SLOTS_PER_EPOCH_TEST`
    /// are pre-Gloas and must be rejected.
    const GLOAS_ACTIVATION_EPOCH: u64 = 1;

    /// Builds a signed ProposerPreferences partial-signature packet with a caller-chosen
    /// `signing_root` and envelope `proposal_slot`. The envelope slot IS the duty's
    /// `proposal_slot` (a possibly-future slot), and it alone governs earliness, lateness,
    /// the proposer-assignment check, and per-`proposal_slot` signing-root dedup. Signed with
    /// `private_key` so it passes signature verification end-to-end.
    fn create_signed_proposer_preferences_message(
        signer_id: OperatorId,
        private_key: &Rsa<Private>,
        proposal_slot: Slot,
        signing_root: Hash256,
    ) -> SignedSSVMessage {
        let partial_sig_messages = PartialSignatureMessages {
            kind: PartialSignatureKind::ProposerPreferences,
            slot: proposal_slot,
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

    /// Builds a ProposerPreferences validation context whose wall clock currently reads
    /// `current_slot`, with `received_at` pinned to the start of that slot. When the packet's
    /// envelope `proposal_slot` equals `current_slot` the message is exactly on time; when
    /// `proposal_slot > current_slot` it is early by `(proposal_slot - current_slot)` slots,
    /// which the earliness tests use to probe `early_slot_allowance`. The Ethereum Gloas fork
    /// is active from epoch 0.
    fn create_proposer_preferences_context<'a>(
        signed_msg: &'a SignedSSVMessage,
        committee_info: &'a crate::CommitteeInfo,
        operator_pub_keys: &'a HashMap<OperatorId, Rsa<Public>>,
        current_slot: Slot,
    ) -> ValidationContext<'a, ManualSlotClock> {
        let now = SystemTime::now();
        let slot_clock = ManualSlotClock::new(
            current_slot,
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

    /// Slot duration used by the ProposerPreferences timing helpers/tests.
    const PROPOSER_PREFERENCES_SLOT_DURATION: Duration = Duration::from_secs(12);

    /// Builds a ProposerPreferences context anchored at genesis slot 0 (start = `genesis`),
    /// with `received_at` set explicitly. Lets the tight-TTL tests place `received_at` exactly
    /// at, and just past, the lateness deadline `slot_start(proposal_slot + LATE_SLOT_ALLOWANCE)
    /// + LATE_MESSAGE_MARGIN`. `slot_start(slot) = genesis + slot * slot_duration`.
    fn create_proposer_preferences_context_at<'a>(
        signed_msg: &'a SignedSSVMessage,
        committee_info: &'a crate::CommitteeInfo,
        operator_pub_keys: &'a HashMap<OperatorId, Rsa<Public>>,
        genesis: SystemTime,
        received_at: SystemTime,
    ) -> ValidationContext<'a, ManualSlotClock> {
        let slot_clock = ManualSlotClock::new(
            Slot::new(0),
            genesis.duration_since(UNIX_EPOCH).unwrap(),
            PROPOSER_PREFERENCES_SLOT_DURATION,
        );

        ValidationContext {
            signed_ssv_message: signed_msg,
            committee_info,
            role: Role::ProposerPreferences,
            received_at,
            slots_per_epoch: SLOTS_PER_EPOCH_TEST,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock,
            operator_pub_keys,
            fork_schedule: generate_fork_schedule(Fork::Boole),
            spec: spec_with_gloas(Some(0)),
        }
    }

    /// Start-of-slot time for a genesis-slot-0 clock: `genesis + slot * slot_duration`.
    fn proposer_preferences_slot_start(genesis: SystemTime, slot: u64) -> SystemTime {
        genesis + PROPOSER_PREFERENCES_SLOT_DURATION * (slot as u32)
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
                ..Default::default()
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
                ..Default::default()
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
        // Retargeted for the reworked TIGHT TTL. The envelope slot is now the duty's
        // `proposal_slot`, so lateness is measured from it with the short
        // `ttl = LATE_SLOT_ALLOWANCE` (2) bucket — NOT the old long
        // `slots_per_epoch + LATE_SLOT_ALLOWANCE` (34) bucket. Accepted through the exact
        // deadline `slot_start(proposal_slot + LATE_SLOT_ALLOWANCE) + LATE_MESSAGE_MARGIN`.
        //
        // The packet built by `create_signed_partial_sig_message` carries proposal_slot = 1.
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

        const PROPOSAL_SLOT: u64 = 1;
        let genesis = SystemTime::now();
        // Deadline: last instant still accepted. `received_at` exactly at the deadline yields
        // zero lateness, which is `<= CLOCK_ERROR_TOLERANCE`, so it is accepted.
        let deadline =
            proposer_preferences_slot_start(genesis, PROPOSAL_SLOT + crate::LATE_SLOT_ALLOWANCE)
                + crate::LATE_MESSAGE_MARGIN;

        let validation_context = create_proposer_preferences_context_at(
            &signed_msg,
            &committee_info,
            &map,
            genesis,
            deadline, // received exactly at the deadline
        );

        // Act
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider::default()),
        );

        // Assert
        assert!(
            result.is_ok(),
            "Expected ProposerPreferences at the tight-TTL deadline to be accepted, got: {result:?}"
        );
    }

    #[test]
    fn test_proposer_preferences_beyond_ttl_rejected() {
        // Retargeted for the reworked TIGHT TTL. One slot-duration past the deadline
        // `slot_start(proposal_slot + LATE_SLOT_ALLOWANCE) + LATE_MESSAGE_MARGIN` is late.
        // This is the explicit boundary partner of the accepted case above: the previous
        // long-bucket value (34 slots) would now be far past the tight deadline.
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

        const PROPOSAL_SLOT: u64 = 1;
        let genesis = SystemTime::now();
        let deadline =
            proposer_preferences_slot_start(genesis, PROPOSAL_SLOT + crate::LATE_SLOT_ALLOWANCE)
                + crate::LATE_MESSAGE_MARGIN;
        // One full slot past the deadline is unambiguously beyond CLOCK_ERROR_TOLERANCE.
        let received_at = deadline + PROPOSER_PREFERENCES_SLOT_DURATION;

        let validation_context = create_proposer_preferences_context_at(
            &signed_msg,
            &committee_info,
            &map,
            genesis,
            received_at,
        );

        // Act
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider::default()),
        );

        // Assert
        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::LateSlotMessage { .. }),
            "LateSlotMessage (ProposerPreferences past tight TTL)",
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
                ..Default::default()
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

    // Earliness allowance is `(1 + spec.min_seed_lookahead) * slots_per_epoch` slots. The test
    // spec is mainnet, where `min_seed_lookahead = 1`, so the allowance is `2 * slots_per_epoch`
    // (= 64 with SLOTS_PER_EPOCH_TEST = 32). A ProposerPreferences `proposal_slot` this far in
    // the future is accepted; one slot further out is `EarlySlotMessage`.
    const PROPOSER_PREFERENCES_EARLINESS_ALLOWANCE_SLOTS: u64 = 2 * SLOTS_PER_EPOCH_TEST;

    #[test]
    fn test_proposer_preferences_future_proposal_slot_within_allowance_accepted() {
        // The envelope slot IS the duty's `proposal_slot`. A future `proposal_slot` up to the
        // proposer lookahead (`(1 + min_seed_lookahead) * slots_per_epoch` slots) ahead of the
        // current slot must be accepted, because a proposer commits preferences for its whole
        // lookahead of future proposal slots at once.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);

        let current_slot = Slot::new(1);
        // Envelope `proposal_slot` exactly at the far edge of the allowance.
        let proposal_slot =
            current_slot + Slot::new(PROPOSER_PREFERENCES_EARLINESS_ALLOWANCE_SLOTS);
        let signed_msg = create_signed_proposer_preferences_message(
            OperatorId(1),
            &private_key,
            proposal_slot,
            Hash256::from([1u8; 32]),
        );

        // Ring must span the lookahead so the future-slot write does not collide; size it
        // like production's ProposerPreferences ring.
        let ring = ((1 + 1) * SLOTS_PER_EPOCH_TEST + 2 * SLOTS_PER_EPOCH_TEST) as usize;
        let validation_context =
            create_proposer_preferences_context(&signed_msg, &committee_info, &map, current_slot);

        // Act
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(ring),
            Arc::new(MockDutiesProvider::default()),
        );

        // Assert: accepted; specifically NOT rejected as early.
        assert!(
            result.is_ok(),
            "Expected future proposal_slot within the earliness allowance to be accepted, got: {result:?}"
        );
    }

    #[test]
    fn test_proposer_preferences_future_proposal_slot_beyond_allowance_early() {
        // One slot past the earliness allowance must be rejected as `EarlySlotMessage`.
        // This is the boundary partner of the accepted case above.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);

        let current_slot = Slot::new(1);
        let proposal_slot =
            current_slot + Slot::new(PROPOSER_PREFERENCES_EARLINESS_ALLOWANCE_SLOTS + 1);
        let signed_msg = create_signed_proposer_preferences_message(
            OperatorId(1),
            &private_key,
            proposal_slot,
            Hash256::from([1u8; 32]),
        );

        let ring = ((1 + 1) * SLOTS_PER_EPOCH_TEST + 2 * SLOTS_PER_EPOCH_TEST) as usize;
        let validation_context =
            create_proposer_preferences_context(&signed_msg, &committee_info, &map, current_slot);

        // Act
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(ring),
            Arc::new(MockDutiesProvider::default()),
        );

        // Assert
        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::EarlySlotMessage { .. }),
            "EarlySlotMessage (ProposerPreferences one slot past the earliness allowance)",
        );
    }

    #[test]
    fn test_non_proposer_preferences_future_slot_still_early() {
        // Regression pin: the earliness allowance is STRICTLY role-scoped to
        // ProposerPreferences (`early_slot_allowance` returns `Duration::ZERO` for every
        // other role). A non-role-8 message (ValidatorRegistration) with a future envelope
        // slot must still be rejected as early — even one slot ahead, which is well inside
        // the ProposerPreferences allowance but not available to any other role.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);

        // ValidatorRegistration packet whose envelope slot is 1.
        let signed_msg = create_signed_partial_sig_message(
            Role::ValidatorRegistration,
            PartialSignatureKind::ValidatorRegistration,
            OperatorId(1),
            &private_key,
        );

        // Clock currently reads slot 0, so the envelope slot 1 is one slot in the future.
        let now = SystemTime::now();
        let slot_clock = ManualSlotClock::new(
            Slot::new(0),
            now.duration_since(UNIX_EPOCH).unwrap(),
            Duration::from_secs(12),
        );
        let validation_context = ValidationContext {
            signed_ssv_message: &signed_msg,
            committee_info: &committee_info,
            role: Role::ValidatorRegistration,
            received_at: now, // received at slot-0 start; envelope slot 1 is 12s early
            slots_per_epoch: SLOTS_PER_EPOCH_TEST,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock,
            operator_pub_keys: &map,
            fork_schedule: generate_fork_schedule(Fork::Alan),
            spec: spec_with_gloas(None),
        };

        // Act
        let result = validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider::default()),
        );

        // Assert
        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::EarlySlotMessage { .. }),
            "EarlySlotMessage (non-ProposerPreferences role has no future-slot allowance)",
        );
    }

    #[test]
    fn envelope_proposer_future_slot_still_early() {
        // `EnvelopeProposer` message slot is its PRESENT emission slot (no future-slot allowance).
        // This case is rejected.

        // Creates `EnvelopeProposer` packet with an envelope slot of 1.
        let (committee_info, private_key, map) = four_node_committee_and_keypair();
        let signed_msg = create_signed_envelope_proposer_message(
            OperatorId(1),
            &private_key,
            Slot::new(1),
            Hash256::from([0x44; 32]),
        );
        // Current slot set to 0. Envelope slot 1 = one slot (12s) in the future.
        let ctx =
            create_envelope_proposer_context(&signed_msg, &committee_info, &map, Slot::new(0));

        // Validate the message and raise the error.
        let result = validate_partial_signature_message(
            ctx,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider::default()),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::EarlySlotMessage { .. }),
            "EarlySlotMessage (EnvelopeProposer has no future-slot allowance. One slot ahead is early)",
        );
    }

    #[test]
    fn test_proposer_preferences_distinct_roots_same_proposal_slot_accepted() {
        // A proposer legitimately signs multiple distinct preference roots for one
        // `proposal_slot` when its inputs change between emissions (chiefly a
        // `dependent_root` shift under reorg). Two packets from the same
        // (validator, operator, proposal_slot) with DIFFERENT signing roots must BOTH be
        // accepted through a shared DutyState, well under the distinct-root cap of
        // `MAX_PROPOSER_PREFERENCES_DISTINCT_ROOTS`.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signer_id = OperatorId(1);
        let proposal_slot = Slot::new(1);
        let mut duty_state = DutyState::new(64);

        // First packet: root A.
        let signed_a = create_signed_proposer_preferences_message(
            signer_id,
            &private_key,
            proposal_slot,
            Hash256::from([0xAA; 32]),
        );
        let context_a =
            create_proposer_preferences_context(&signed_a, &committee_info, &map, proposal_slot);
        let result_a = validate_partial_signature_message(
            context_a,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );
        assert!(
            result_a.is_ok(),
            "Expected first distinct-root packet to be accepted, got: {result_a:?}"
        );

        // Second packet: DIFFERENT root B at the SAME proposal_slot.
        let signed_b = create_signed_proposer_preferences_message(
            signer_id,
            &private_key,
            proposal_slot,
            Hash256::from([0xBB; 32]),
        );
        let context_b =
            create_proposer_preferences_context(&signed_b, &committee_info, &map, proposal_slot);
        let result_b = validate_partial_signature_message(
            context_b,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );

        // Assert: the second distinct root is also accepted (NOT falsely rejected
        // as a duplicate).
        assert!(
            result_b.is_ok(),
            "Expected second distinct-root packet at same proposal_slot to be accepted, got: {result_b:?}"
        );
    }

    #[test]
    fn test_proposer_preferences_duplicate_root_same_proposal_slot_rejected() {
        // An exact-duplicate signing_root for the same `proposal_slot` is a resend and
        // must be rejected as a `DuplicatedMessage` (Reject class), via the per-slot
        // `seen_preferences` set.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signer_id = OperatorId(1);
        let proposal_slot = Slot::new(1);
        let root = Hash256::from([0xCC; 32]);
        let mut duty_state = DutyState::new(64);

        // First packet with root R is accepted.
        let signed_first = create_signed_proposer_preferences_message(
            signer_id,
            &private_key,
            proposal_slot,
            root,
        );
        let context_first = create_proposer_preferences_context(
            &signed_first,
            &committee_info,
            &map,
            proposal_slot,
        );
        let result_first = validate_partial_signature_message(
            context_first,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );
        assert!(
            result_first.is_ok(),
            "Expected first packet to be accepted, got: {result_first:?}"
        );

        // Second packet: EXACT-DUPLICATE root R at the SAME proposal_slot.
        let signed_dup = create_signed_proposer_preferences_message(
            signer_id,
            &private_key,
            proposal_slot,
            root,
        );
        let context_dup =
            create_proposer_preferences_context(&signed_dup, &committee_info, &map, proposal_slot);
        let result_dup = validate_partial_signature_message(
            context_dup,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
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
        // Retargeted for the reworked cap. The distinct-root set for one
        // (validator, operator, proposal_slot) is capped at
        // `MAX_PROPOSER_PREFERENCES_DISTINCT_ROOTS` (referenced, never hardcoded). Feeding
        // exactly that many distinct roots all pass; one more distinct root is an Ignore-class
        // `TooManyDistinctSigningRoots` (NOT the old `InvalidPartialSignatureTypeCount`).
        use crate::duty_state::MAX_PROPOSER_PREFERENCES_DISTINCT_ROOTS_FOR_TEST as CAP;

        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signer_id = OperatorId(1);
        let proposal_slot = Slot::new(1);
        let mut duty_state = DutyState::new(64);

        // Feed `CAP` distinct roots; all must be accepted.
        for i in 0..CAP {
            let mut root_bytes = [0u8; 32];
            root_bytes[0..8].copy_from_slice(&(i as u64).to_le_bytes());
            let signed = create_signed_proposer_preferences_message(
                signer_id,
                &private_key,
                proposal_slot,
                Hash256::from(root_bytes),
            );
            let context =
                create_proposer_preferences_context(&signed, &committee_info, &map, proposal_slot);
            let result = validate_partial_signature_message(
                context,
                &mut duty_state,
                Arc::new(MockDutiesProvider::default()),
            );
            assert!(
                result.is_ok(),
                "Expected distinct root #{i} (within cap {CAP}) to be accepted, got: {result:?}"
            );
        }

        // While the set is exactly full (CAP distinct roots), a resend of the FIRST already-seen
        // root must be a Reject-class `DuplicatedMessage` — identity takes precedence over the cap,
        // NOT the Ignore-class `TooManyDistinctSigningRoots`.
        let mut first_root_bytes = [0u8; 32];
        first_root_bytes[0..8].copy_from_slice(&0u64.to_le_bytes());
        let signed_dup = create_signed_proposer_preferences_message(
            signer_id,
            &private_key,
            proposal_slot,
            Hash256::from(first_root_bytes),
        );
        let context_dup =
            create_proposer_preferences_context(&signed_dup, &committee_info, &map, proposal_slot);
        let result_dup = validate_partial_signature_message(
            context_dup,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );
        assert_validation_error(
            result_dup,
            |failure| matches!(failure, ValidationFailure::DuplicatedMessage { .. }),
            "DuplicatedMessage (already-seen root takes precedence over cap)",
        );
        // Pin the Reject mapping so a future reclassification is caught here. `MessageAcceptance`
        // has no `PartialEq`, so match on the variant.
        assert!(
            matches!(
                MessageAcceptance::from(&ValidationFailure::DuplicatedMessage {
                    got: String::new()
                }),
                MessageAcceptance::Reject
            ),
            "DuplicatedMessage must map to Reject"
        );

        // One more distinct root exceeds the cap and is rejected as an Ignore.
        let mut over_cap_bytes = [0u8; 32];
        over_cap_bytes[0..8].copy_from_slice(&(CAP as u64).to_le_bytes());
        let signed_over = create_signed_proposer_preferences_message(
            signer_id,
            &private_key,
            proposal_slot,
            Hash256::from(over_cap_bytes),
        );
        let context_over =
            create_proposer_preferences_context(&signed_over, &committee_info, &map, proposal_slot);
        let result_over = validate_partial_signature_message(
            context_over,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );

        // Assert
        assert_validation_error(
            result_over,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::TooManyDistinctSigningRoots { .. }
                )
            },
            "TooManyDistinctSigningRoots (ProposerPreferences distinct-root cap exceeded)",
        );
    }

    #[test]
    fn test_proposer_preferences_earlier_proposal_slot_after_later_accepted() {
        // Slot-advance exemption: ProposerPreferences is NOT a `monotonic_slot_role()`, because
        // a signer holds its whole lookahead of proposal slots at once. After a role-8 message
        // for a LATER `proposal_slot`, a role-8 message for an EARLIER `proposal_slot` (still
        // inside the lateness window) must be ACCEPTED — the stale-slot rejection does not apply.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signer_id = OperatorId(1);
        let later_slot = Slot::new(3);
        let earlier_slot = Slot::new(2);
        let mut duty_state = DutyState::new(64);

        // Genesis-slot-0 clock so `slot_start(earlier_slot)` is representable; `received_at` at
        // the later slot's start keeps both messages within earliness/lateness.
        let genesis = SystemTime::now();
        let received_at = proposer_preferences_slot_start(genesis, later_slot.as_u64());

        // First: process the LATER proposal_slot (advances the operator's max_slot).
        let signed_later = create_signed_proposer_preferences_message(
            signer_id,
            &private_key,
            later_slot,
            Hash256::from([0x11; 32]),
        );
        let context_later = create_proposer_preferences_context_at(
            &signed_later,
            &committee_info,
            &map,
            genesis,
            received_at,
        );
        let result_later = validate_partial_signature_message(
            context_later,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );
        assert!(
            result_later.is_ok(),
            "Expected later proposal_slot message to be accepted, got: {result_later:?}"
        );

        // Then: an EARLIER proposal_slot must still be accepted (no `SlotAlreadyAdvanced`).
        let signed_earlier = create_signed_proposer_preferences_message(
            signer_id,
            &private_key,
            earlier_slot,
            Hash256::from([0x22; 32]),
        );
        let context_earlier = create_proposer_preferences_context_at(
            &signed_earlier,
            &committee_info,
            &map,
            genesis,
            received_at,
        );
        let result_earlier = validate_partial_signature_message(
            context_earlier,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );

        // Assert
        assert!(
            result_earlier.is_ok(),
            "Expected earlier proposal_slot after a later one to be accepted (role-8 is not \
             monotonic), got: {result_earlier:?}"
        );
    }

    #[test]
    fn test_monotonic_role_earlier_slot_after_later_rejected() {
        // Contrast to the ProposerPreferences exemption above: a monotonic-slot role
        // (Aggregator) that has advanced to a later slot must REJECT an earlier slot with
        // `SlotAlreadyAdvanced`. The slot-advance check runs before timing, so a lenient
        // pre-Boole fork and long TTL keep the earlier message reaching that check.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signer_id = OperatorId(1);
        let mut duty_state = DutyState::new(64);

        // Pre-advance the operator's max_slot to slot 5 with a dummy Aggregator message.
        let dummy = PartialSignatureMessages {
            kind: PartialSignatureKind::SelectionProofPartialSig,
            slot: Slot::new(5),
            messages: VariableList::new(vec![PartialSignatureMessage {
                partial_signature: Signature::empty(),
                signing_root: Hash256::from([0u8; 32]),
                signer: signer_id,
                validator_index: ValidatorIndex(0),
            }])
            .unwrap(),
        };
        duty_state
            .update_for_partial_signature(&dummy, &signer_id, SLOTS_PER_EPOCH_TEST)
            .unwrap();

        // Now an Aggregator message for an EARLIER slot (2) must be rejected.
        let (mut earlier_msgs, _) = create_test_partial_signature(
            Role::Aggregator,
            PartialSignatureKind::SelectionProofPartialSig,
            signer_id,
            PartialSigTestOptions::default(),
            Some(private_key.clone()),
        );
        earlier_msgs.slot = Slot::new(2);
        let msg_id = create_message_id_for_test(Role::Aggregator);
        let ssv_msg = SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            msg_id,
            earlier_msgs.as_ssz_bytes(),
        )
        .unwrap();
        let p_key = PKey::from_rsa(private_key).unwrap();
        let mut s = Signer::new(MessageDigest::sha256(), &p_key).unwrap();
        s.update(&ssv_msg.as_ssz_bytes()).unwrap();
        let signature = s.sign_to_vec().unwrap().try_into().unwrap();
        let signed_msg =
            SignedSSVMessage::new(vec![signature], vec![signer_id], ssv_msg, vec![]).unwrap();

        let now = SystemTime::now();
        let slot_clock = ManualSlotClock::new(
            Slot::new(2),
            now.duration_since(UNIX_EPOCH).unwrap(),
            Duration::from_secs(12),
        );
        let validation_context = ValidationContext {
            signed_ssv_message: &signed_msg,
            committee_info: &committee_info,
            role: Role::Aggregator,
            received_at: now,
            slots_per_epoch: SLOTS_PER_EPOCH_TEST,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock,
            operator_pub_keys: &map,
            // Pre-Boole: Aggregator is still an active role.
            fork_schedule: generate_fork_schedule(Fork::Alan),
            spec: spec_with_gloas(None),
        };

        let result = validate_partial_signature_message(
            validation_context,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );

        // Assert
        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::SlotAlreadyAdvanced { .. }),
            "SlotAlreadyAdvanced (monotonic role rejects an earlier slot)",
        );
    }

    // ==================== ProposerPreferences proposer-assignment arm ====================
    //
    // `validate_beacon_duty` gained a `Role::ProposerPreferences` arm keyed on the envelope
    // `proposal_slot`. It rejects with `NoDuty` (Ignore) only when the slot's epoch is known
    // AND the validator is NOT the assigned proposer; an unknown epoch is tolerated (accepted),
    // and no RANDAO tolerance applies. These three cases drive the mock's
    // `is_epoch_known_for_proposers` / `is_validator_proposer_at_slot`.

    /// Runs the full ProposerPreferences pipeline with a mock configured for the given
    /// proposer-assignment knobs, returning the validation result.
    fn run_proposer_preferences_with_duties(
        epoch_known_for_proposers: bool,
        validator_is_proposer: bool,
    ) -> Result<ValidatedSSVMessage, ValidationFailure> {
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signed_msg = create_signed_proposer_preferences_message(
            OperatorId(1),
            &private_key,
            Slot::new(1),
            Hash256::from([0x33; 32]),
        );
        let validation_context =
            create_proposer_preferences_context(&signed_msg, &committee_info, &map, Slot::new(1));

        validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                // Map the two legacy knobs onto the pubkey-keyed lookup the arm now uses:
                // an unfetched epoch is `None` (tolerated), a fetched epoch reports
                // `Some(is the validator the assigned proposer)`.
                proposer_assignment: proposer_assignment_from_knobs(
                    epoch_known_for_proposers,
                    validator_is_proposer,
                ),
                ..Default::default()
            }),
        )
    }

    /// Maps the legacy `is_epoch_known_for_proposers` / `is_validator_proposer_at_slot`
    /// knobs onto the `proposer_assignment_at_slot` return the arm now consumes:
    /// unknown epoch -> `None` (tolerated), known epoch -> `Some(assigned?)`.
    fn proposer_assignment_from_knobs(epoch_known: bool, is_proposer: bool) -> Option<bool> {
        epoch_known.then_some(is_proposer)
    }

    #[test]
    fn test_proposer_preferences_assigned_proposer_accepted() {
        // Known epoch + validator IS the assigned proposer → accepted.
        let result = run_proposer_preferences_with_duties(true, true);
        assert!(
            result.is_ok(),
            "Expected assigned proposer to be accepted, got: {result:?}"
        );
    }

    #[test]
    fn test_proposer_preferences_known_epoch_not_assigned_no_duty() {
        // Known epoch + validator is NOT the assigned proposer → `NoDuty` (maps to Ignore).
        let result = run_proposer_preferences_with_duties(true, false);
        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::NoDuty),
            "NoDuty (known epoch, validator not the assigned proposer)",
        );
        // Pin the Ignore mapping so a future reclassification of NoDuty is caught here.
        // `MessageAcceptance` does not implement `PartialEq`, so match on the variant.
        assert!(
            matches!(
                MessageAcceptance::from(&ValidationFailure::NoDuty),
                MessageAcceptance::Ignore
            ),
            "NoDuty must map to Ignore"
        );
    }

    #[test]
    fn test_proposer_preferences_unknown_epoch_tolerated() {
        // Unknown epoch → the assignment check is skipped and the message is accepted
        // (tolerated while proposer duties for that epoch are not yet fetched). The
        // not-assigned flag is irrelevant here because the `&&` short-circuits on the
        // unknown epoch.
        let result = run_proposer_preferences_with_duties(false, false);
        assert!(
            result.is_ok(),
            "Expected unknown-epoch ProposerPreferences to be tolerated (accepted), got: {result:?}"
        );
    }

    /// Runs the full ProposerPreferences pipeline, driving `proposer_assignment_at_slot`
    /// DIRECTLY with `proposer_assignment` (rather than via the legacy knob mapping) and
    /// allowing the caller to supply the `committee_info`. This pins the pubkey-keyed arm
    /// introduced in #1142, which no longer consults `committee_info.validator_indices`.
    fn run_proposer_preferences_with_assignment(
        committee_info: crate::CommitteeInfo,
        proposer_assignment: Option<bool>,
    ) -> Result<ValidatedSSVMessage, ValidationFailure> {
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signed_msg = create_signed_proposer_preferences_message(
            OperatorId(1),
            &private_key,
            Slot::new(1),
            Hash256::from([0x33; 32]),
        );
        let validation_context =
            create_proposer_preferences_context(&signed_msg, &committee_info, &map, Slot::new(1));

        validate_partial_signature_message(
            validation_context,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider {
                proposer_assignment,
                ..Default::default()
            }),
        )
    }

    #[test]
    fn test_proposer_preferences_assigned_pubkey_accepted_with_unresolved_local_index() {
        // #1142: the `ProposerPreferences` arm is keyed on the message-id validator PUBKEY via
        // `proposer_assignment_at_slot`, and no longer reads `committee_info.validator_indices`.
        // A locally-unresolved validator index (empty `validator_indices`) must therefore NOT
        // block an otherwise-assigned proposer. Before #1142 the index-based path would have
        // failed to find a validator index and rejected the message.

        // Arrange: committee with members but NO resolved local validator indices, and a mock
        // reporting the pubkey IS the assigned proposer at the slot (`Some(true)`).
        let committee_info = crate::CommitteeInfo {
            committee_members: create_committee_info(FOUR_NODE_COMMITTEE).committee_members,
            validator_indices: vec![],
        };

        // Act
        let result = run_proposer_preferences_with_assignment(committee_info, Some(true));

        // Assert: accepted purely on the pubkey-keyed assignment, index resolution irrelevant.
        assert!(
            result.is_ok(),
            "Expected assigned pubkey to be accepted despite an unresolved local validator \
             index, got: {result:?}"
        );
    }

    #[test]
    fn test_proposer_preferences_assignment_some_true_accepted() {
        // #1142: `proposer_assignment_at_slot` == `Some(true)` (assigned proposer) -> accepted.
        // Arrange
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        // Act
        let result = run_proposer_preferences_with_assignment(committee_info, Some(true));
        // Assert
        assert!(
            result.is_ok(),
            "Expected `Some(true)` assignment to be accepted, got: {result:?}"
        );
    }

    #[test]
    fn test_proposer_preferences_assignment_some_false_no_duty_maps_to_ignore() {
        // #1142: `proposer_assignment_at_slot` == `Some(false)` (fetched epoch proves the pubkey
        // is NOT the proposer at this slot) -> `NoDuty`, which must map to `Ignore`.
        // Arrange
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        // Act
        let result = run_proposer_preferences_with_assignment(committee_info, Some(false));
        // Assert
        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::NoDuty),
            "NoDuty (Some(false): fetched epoch, pubkey not the assigned proposer)",
        );
        assert!(
            matches!(
                MessageAcceptance::from(&ValidationFailure::NoDuty),
                MessageAcceptance::Ignore
            ),
            "NoDuty must map to Ignore"
        );
    }

    #[test]
    fn test_proposer_preferences_assignment_none_tolerated() {
        // #1142: `proposer_assignment_at_slot` == `None` (slot's epoch not fetched / unknown) ->
        // tolerated (accepted). Only `Some(false)` rejects.
        // Arrange
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        // Act
        let result = run_proposer_preferences_with_assignment(committee_info, None);
        // Assert
        assert!(
            result.is_ok(),
            "Expected `None` (unfetched epoch) assignment to be tolerated (accepted), got: \
             {result:?}"
        );
    }

    #[test]
    fn test_proposer_role_still_uses_index_path_not_pubkey_assignment() {
        // Regression pin for #1142: the INDEX-based `Role::Proposer` arm of `validate_beacon_duty`
        // is unchanged. It must decide purely on `is_validator_proposer_at_slot` (the index-keyed
        // lookup), independent of the new pubkey-keyed `proposer_assignment_at_slot`. We prove the
        // separation by driving the two lookups to OPPOSITE verdicts:
        //   - `proposer_assignment = Some(true)` (the pubkey arm would ACCEPT), yet
        //   - `validator_is_proposer = false`  (the index arm must REJECT with `NoDuty`).
        // The Proposer path must reject, showing it never consulted the pubkey assignment.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (_, signed_msg) = create_test_partial_signature(
            Role::Proposer,
            PartialSignatureKind::PostConsensus,
            OperatorId(1),
            PartialSigTestOptions::default(),
            None,
        );
        let binding = generate_random_rsa_public_keys(signed_msg.operator_ids().len());
        let map = create_operator_pub_keys(committee_info.committee_members.clone(), binding);
        let validation_context = create_test_validation_context(
            &signed_msg,
            &committee_info,
            Role::Proposer,
            &map,
            generate_fork_schedule(Fork::Alan),
        );

        // Act: `randao_msg = false` so the Proposer arm goes straight to the index check.
        let result = validate_beacon_duty(
            &validation_context,
            Slot::new(0),
            false,
            Arc::new(MockDutiesProvider {
                validator_is_proposer: false,
                proposer_assignment: Some(true),
                ..Default::default()
            }),
        );

        // Assert: rejected by the index arm; the pubkey `Some(true)` did not rescue it.
        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::NoDuty),
            "NoDuty (Role::Proposer index path unchanged, ignores pubkey assignment)",
        );
    }

    #[test]
    fn test_proposer_preferences_ring_avoids_lookahead_collision() {
        // Mirrors go's TestProposerPreferencesRingAvoidsLookaheadCollision. The
        // ProposerPreferences `DutyState` ring is sized to cover the proposer lookahead so a
        // validly accepted future-slot write cannot evict a live slot's dedup state. Two
        // accepted role-8 messages — one for the current slot S and one for S + 2*spe (the
        // earliness bound) — must retain DISTINCT per-slot signer states. Concretely, after the
        // S + 2*spe message, a NEW distinct-root at S is still accepted (its `seen_preferences`
        // survived), rather than being reset by a ring-index collision.
        let committee_info = create_committee_info(FOUR_NODE_COMMITTEE);
        let (private_key, public_key) = generate_test_key_pair();
        let map =
            create_operator_pub_keys(committee_info.committee_members.clone(), vec![public_key]);
        let signer_id = OperatorId(1);

        // Exercise the production ring-size selector so this test fails if the role-8 arm ever
        // loses its lookahead-sized ring. `spec_with_gloas(None)` is mainnet-based
        // (`min_seed_lookahead` = 1), so the selected ring is (1 + 1) * spe + 2 * spe = 128.
        let spec = spec_with_gloas(None);
        let ring = crate::stored_slot_count(Role::ProposerPreferences, SLOTS_PER_EPOCH_TEST, &spec);
        let mut duty_state = DutyState::new(ring);

        // Guard: a representative non-role-8 role keeps the plain two-epoch default, confirming the
        // lookahead expansion is specific to `ProposerPreferences`.
        assert_eq!(
            crate::stored_slot_count(Role::Proposer, SLOTS_PER_EPOCH_TEST, &spec),
            (2 * SLOTS_PER_EPOCH_TEST) as usize,
        );

        let slot_s = Slot::new(1);
        let far_slot = slot_s + Slot::new(2 * SLOTS_PER_EPOCH_TEST); // earliness bound
        // Note: slot_s and far_slot differ by exactly 2*spe = 64. The ring length is 128, so
        // they map to DISTINCT indices only because the ring covers the whole lookahead; a
        // 2*spe-sized ring (64) would alias them and this test would fail.

        // 1) Accept a first root at slot S.
        let signed_s1 = create_signed_proposer_preferences_message(
            signer_id,
            &private_key,
            slot_s,
            Hash256::from([0xA1; 32]),
        );
        let ctx_s1 = create_proposer_preferences_context(&signed_s1, &committee_info, &map, slot_s);
        assert!(
            validate_partial_signature_message(
                ctx_s1,
                &mut duty_state,
                Arc::new(MockDutiesProvider::default()),
            )
            .is_ok(),
            "Expected first root at slot S to be accepted"
        );

        // 2) Accept a root at the far slot S + 2*spe (still within the earliness allowance).
        let signed_far = create_signed_proposer_preferences_message(
            signer_id,
            &private_key,
            far_slot,
            Hash256::from([0xB1; 32]),
        );
        // Current slot stays S; the far slot is a future proposal_slot inside the allowance.
        let ctx_far =
            create_proposer_preferences_context(&signed_far, &committee_info, &map, slot_s);
        assert!(
            validate_partial_signature_message(
                ctx_far,
                &mut duty_state,
                Arc::new(MockDutiesProvider::default()),
            )
            .is_ok(),
            "Expected root at far slot S + 2*spe to be accepted"
        );

        // 3) A NEW distinct root at slot S must still be accepted: slot S's `seen_preferences`
        //    survived the far-slot write (no ring-index collision reset it).
        let signed_s2 = create_signed_proposer_preferences_message(
            signer_id,
            &private_key,
            slot_s,
            Hash256::from([0xA2; 32]),
        );
        let ctx_s2 = create_proposer_preferences_context(&signed_s2, &committee_info, &map, slot_s);
        let result_s2 = validate_partial_signature_message(
            ctx_s2,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );

        // Assert: accepted as a genuinely new distinct root (state preserved), not treated as
        // slot S's first message (which a collision-reset would have produced).
        assert!(
            result_s2.is_ok(),
            "Expected a new distinct root at slot S to be accepted (ring preserved S's dedup \
             state across the far-slot write), got: {result_s2:?}"
        );

        // Cross-check that slot S really did accumulate two distinct roots (dedup state alive):
        // an EXACT-duplicate of the first root at S is now rejected as a duplicate.
        let signed_dup = create_signed_proposer_preferences_message(
            signer_id,
            &private_key,
            slot_s,
            Hash256::from([0xA1; 32]), // same as signed_s1
        );
        let ctx_dup =
            create_proposer_preferences_context(&signed_dup, &committee_info, &map, slot_s);
        let result_dup = validate_partial_signature_message(
            ctx_dup,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );
        assert_validation_error(
            result_dup,
            |failure| matches!(failure, ValidationFailure::DuplicatedMessage { .. }),
            "DuplicatedMessage (slot S retained its first root across the far-slot write)",
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
                ..Default::default()
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
                ..Default::default()
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

    // ==================== EnvelopeProposer proposer-assignment arm ====================
    //
    // `validate_beacon_duty`'s `Role::ProposerPreferences | Role::EnvelopeProposer` arm (keyed on
    // the message's `slot`) rejects with `NoDuty` only when the slot's epoch is known AND the
    // validator is NOT the assigned proposer; an unknown epoch is tolerated and no RANDAO
    // tolerance applies.

    /// Runs the `EnvelopeProposer` proposer-assignment arm of `validate_beacon_duty` with the
    /// mock's two knobs, returning the result for the caller to assert on. `randao_msg` is always
    /// false for `EnvelopeProposer` (no RANDAO tolerance applies).
    fn run_envelope_beacon_duty(
        epoch_known: bool,
        is_proposer: bool,
    ) -> Result<(), ValidationFailure> {
        let (committee_info, private_key, map) = four_node_committee_and_keypair();
        let signed_msg = create_signed_envelope_proposer_message(
            OperatorId(1),
            &private_key,
            Slot::new(0),
            Hash256::from([0x33; 32]),
        );
        let validation_context =
            create_envelope_proposer_context(&signed_msg, &committee_info, &map, Slot::new(0));

        validate_beacon_duty(
            &validation_context,
            Slot::new(0),
            false,
            Arc::new(MockDutiesProvider {
                proposer_assignment: proposer_assignment_from_knobs(epoch_known, is_proposer),
                ..Default::default()
            }),
        )
    }

    #[test]
    fn envelope_proposer_tolerates_unknown_proposer_epoch() {
        // Before the epoch's proposer duties are fetched, tolerate (Ok), not drop as NoDuty.
        let result = run_envelope_beacon_duty(false, false);
        assert!(
            result.is_ok(),
            "unknown proposer epoch must be tolerated for EnvelopeProposer, got: {result:?}"
        );
    }

    #[test]
    fn envelope_proposer_known_epoch_non_proposer_is_no_duty() {
        // Known epoch, not the assigned proposer -> reject with NoDuty.
        let result = run_envelope_beacon_duty(true, false);
        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::NoDuty),
            "NoDuty (known epoch, validator not the assigned proposer)",
        );
    }

    #[test]
    fn envelope_proposer_known_epoch_proposer_ok() {
        // Known epoch and the assigned proposer -> accept.
        let result = run_envelope_beacon_duty(true, true);
        assert!(
            result.is_ok(),
            "Expected assigned proposer to be accepted for EnvelopeProposer, got: {result:?}"
        );
    }

    #[test]
    fn envelope_proposer_shared_state_accepts_consensus_then_partial_sig() {
        // Consensus and PostConsensus share one DutyState per MessageId. Validate a consensus
        // message at slot 1, persist it (the outer `validate_consensus_message` does this via
        // `update_for_consensus_message`; `validate_qbft_message_by_duty_logic` alone does not),
        // then validate a same-slot PostConsensus. The guard is strict (`max_slot > message_slot`),
        // so the `max_slot == message_slot` equality boundary must be tolerated (Ok).

        // Arrange: EnvelopeProposer consensus message at slot 1 (the builder defaults height to 1).
        let (committee_info, private_key, map) = four_node_committee_and_keypair();

        let qbft_message = crate::tests::QbftMessageBuilder::new(
            Role::EnvelopeProposer,
            ssv_types::consensus::QbftMessageType::Prepare,
        )
        .build();
        let consensus_signed_msg = crate::tests::create_signed_consensus_message(
            qbft_message.clone(),
            vec![OperatorId(1)],
            vec![],
            vec![private_key.clone()],
        );
        let consensus_context = create_envelope_proposer_context(
            &consensus_signed_msg,
            &committee_info,
            &map,
            Slot::new(1),
        );

        let mut shared_duty_state = crate::duty_state::DutyState::new(64);

        let consensus_result = crate::consensus_message::validate_qbft_message_by_duty_logic(
            &consensus_context,
            &qbft_message,
            &mut shared_duty_state,
            Arc::new(MockDutiesProvider::default()),
        );
        assert!(
            consensus_result.is_ok(),
            "EnvelopeProposer consensus at slot 1 must be accepted"
        );

        // Persist the accepted consensus so `max_slot == 1` before Act 2; without it the state
        // would stay at `max_slot == 0` and never exercise the equality boundary.
        shared_duty_state.update_for_consensus_message(
            &consensus_signed_msg,
            &qbft_message,
            SLOTS_PER_EPOCH_TEST,
        );

        // Act: same-slot PostConsensus partial sig against the now-advanced shared state.
        let partial_sig_signed_msg = create_signed_envelope_proposer_message(
            OperatorId(1),
            &private_key,
            Slot::new(1),
            Hash256::from([0x33; 32]),
        );
        let partial_sig_context = create_envelope_proposer_context(
            &partial_sig_signed_msg,
            &committee_info,
            &map,
            Slot::new(1),
        );
        let partial_sig_result = validate_partial_signature_message(
            partial_sig_context,
            &mut shared_duty_state,
            Arc::new(MockDutiesProvider::default()),
        );

        assert!(
            partial_sig_result.is_ok(),
            "EnvelopeProposer PostConsensus at slot 1 must be accepted when the shared state's max_slot already equals 1 (equality boundary of the strict monotonic guard)"
        );
    }

    /// Seeds `state` with an accepted `EnvelopeProposer` consensus message at `height` for
    /// `OperatorId(1)`, advancing that signer's max slot.
    fn seed_state_via_consensus(state: &mut crate::duty_state::DutyState, height: u64) {
        let mut qbft = crate::tests::QbftMessageBuilder::new(
            Role::EnvelopeProposer,
            ssv_types::consensus::QbftMessageType::Prepare,
        )
        .build();
        qbft.height = height;
        let signed_msg = crate::tests::create_signed_consensus_message(
            qbft.clone(),
            vec![OperatorId(1)],
            vec![],
            vec![],
        );
        state.update_for_consensus_message(&signed_msg, &qbft, SLOTS_PER_EPOCH_TEST);
    }

    #[test]
    fn envelope_proposer_consensus_advances_blocks_lower_partial_sig() {
        // §7 monotonic-slot rule, consensus-advances direction: a consensus message at slot 10
        // must block a later PostConsensus partial sig at the lower slot 5.
        let (committee_info, private_key, map) = four_node_committee_and_keypair();

        // Arrange: Seed DutyState at slot 10 via consensus update.
        let mut duty_state = crate::duty_state::DutyState::new(64);
        seed_state_via_consensus(&mut duty_state, 10);

        // Arrange: PostConsensus partial sig at slot 5 (lower than 10).
        let partial_sig_signed_msg = create_signed_envelope_proposer_message(
            OperatorId(1),
            &private_key,
            Slot::new(5),
            Hash256::from([0x33; 32]),
        );
        let validation_context = create_envelope_proposer_context(
            &partial_sig_signed_msg,
            &committee_info,
            &map,
            Slot::new(10),
        );

        // Act: Validate PostConsensus partial sig at slot 5 against advanced DutyState.
        let result = validate_partial_signature_message(
            validation_context,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );

        // Assert: Must be rejected as SlotAlreadyAdvanced.
        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    crate::ValidationFailure::SlotAlreadyAdvanced { .. }
                )
            },
            "EnvelopeProposer PostConsensus below advanced consensus slot must be SlotAlreadyAdvanced",
        );
    }

    #[test]
    fn envelope_proposer_partial_sig_advances_blocks_lower_consensus() {
        // §7 monotonic-slot rule, partial-sig-advances direction: seeding via a PostConsensus
        // partial sig at slot 10 must block a later consensus message at the lower height 5.
        // The seeding mechanism (partial sig, not consensus) is the point of this direction.
        let (committee_info, private_key, map) = four_node_committee_and_keypair();

        // Arrange: Seed DutyState at slot 10 via partial-sig update.
        let mut duty_state = crate::duty_state::DutyState::new(64);
        let dummy_partial_sig = create_signed_envelope_proposer_message(
            OperatorId(1),
            &private_key,
            Slot::new(10),
            Hash256::from([0x99; 32]),
        );
        let messages =
            PartialSignatureMessages::from_ssz_bytes(dummy_partial_sig.ssv_message().data())
                .expect("dummy envelope message must decode");
        duty_state
            .update_for_partial_signature(&messages, &OperatorId(1), SLOTS_PER_EPOCH_TEST)
            .expect("seeding partial-signature state must succeed");

        // Arrange: Consensus message at height 5 (lower than 10).
        let mut qbft_message = crate::tests::QbftMessageBuilder::new(
            Role::EnvelopeProposer,
            ssv_types::consensus::QbftMessageType::Prepare,
        )
        .build();
        qbft_message.height = 5;
        let consensus_signed_msg = crate::tests::create_signed_consensus_message(
            qbft_message.clone(),
            vec![OperatorId(1)],
            vec![],
            vec![private_key.clone()],
        );
        let validation_context = create_envelope_proposer_context(
            &consensus_signed_msg,
            &committee_info,
            &map,
            Slot::new(10),
        );

        // Act: Validate consensus message at height 5 against advanced DutyState.
        let result = crate::consensus_message::validate_qbft_message_by_duty_logic(
            &validation_context,
            &qbft_message,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );

        // Assert: Must be rejected as SlotAlreadyAdvanced.
        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    crate::ValidationFailure::SlotAlreadyAdvanced { .. }
                )
            },
            "EnvelopeProposer consensus below advanced PostConsensus slot must be SlotAlreadyAdvanced",
        );
    }

    #[test]
    fn envelope_proposer_repeated_post_consensus_rejected() {
        // Validate an EnvelopeProposer PostConsensus at slot 5 (accepted; records the
        // post-consensus count), then validate a second PostConsensus for the same
        // signer/slot against the same DutyState. The inherited post-consensus seen-message
        // guard must reject the second as InvalidPartialSignatureTypeCount.

        let (committee_info, private_key, map) = four_node_committee_and_keypair();

        // Arrange: First PostConsensus message at slot 5.
        let first_signed_msg = create_signed_envelope_proposer_message(
            OperatorId(1),
            &private_key,
            Slot::new(5),
            Hash256::from([0x33; 32]),
        );
        let first_context = create_envelope_proposer_context(
            &first_signed_msg,
            &committee_info,
            &map,
            Slot::new(5),
        );

        let mut duty_state = crate::duty_state::DutyState::new(64);

        let first_result = validate_partial_signature_message(
            first_context,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );
        assert!(
            first_result.is_ok(),
            "First EnvelopeProposer PostConsensus must be accepted"
        );

        // A second PostConsensus for the same signer/slot (only the root differs).
        let second_signed_msg = create_signed_envelope_proposer_message(
            OperatorId(1),
            &private_key,
            Slot::new(5),
            Hash256::from([0x44; 32]),
        );
        let second_context = create_envelope_proposer_context(
            &second_signed_msg,
            &committee_info,
            &map,
            Slot::new(5),
        );
        let second_result = validate_partial_signature_message(
            second_context,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );

        assert_validation_error(
            second_result,
            |failure| {
                matches!(
                    failure,
                    crate::ValidationFailure::InvalidPartialSignatureTypeCount { .. }
                )
            },
            "Repeated EnvelopeProposer PostConsensus for the same signer/slot must be rejected",
        );
    }
}

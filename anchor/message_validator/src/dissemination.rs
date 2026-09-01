use std::sync::Arc;

use duties_tracker::DutiesProvider;
use slot_clock::SlotClock;
use ssv_types::{dissemination::EnvelopeDissemination, msgid::Role};
use ssz::Decode;

use crate::{
    ValidatedSSVMessage, ValidationContext, ValidationFailure, duty_state::DutyState,
    validate_beacon_duty, validate_duty_count, validate_role_for_fork, validate_slot_time,
    verify_single_signer,
};

/// Validates an envelope dissemination message (SIP-94 §6/§7).
///
/// The class is structural-only at this layer: the checks binding the disseminated envelope
/// to the block-QBFT decision are runner concerns, and validation never judges the envelope's
/// content. Dedup is first-valid per (`MessageId`, slot): the first message passing all other
/// rules is recorded, and further dissemination messages for the tuple are Ignore regardless
/// of content or peer (an honest origin retry can repeat one after the recipient's gossip
/// duplicate cache expires, so repetition does not prove peer fault).
///
/// First-valid means first STRUCTURALLY valid, by design: SIP-94 accepts that a Byzantine
/// committee member can consume a slot's dissemination budget with a decision-unbound or
/// payload-unbound carrier, costing at most one missed self-build reveal (never a wrong
/// payload on chain). Do not move semantic rejection into a replacement-capable store; the
/// named hardening for that trade is sign-all, a protocol change.
pub(crate) fn validate_envelope_dissemination(
    validation_context: ValidationContext<impl SlotClock>,
    duty_state: &mut DutyState,
    duty_provider: Arc<impl DutiesProvider>,
) -> Result<ValidatedSSVMessage, ValidationFailure> {
    // Rule: dissemination messages are admitted only for `Role::EnvelopeProposer`.
    if validation_context.role != Role::EnvelopeProposer {
        return Err(ValidationFailure::UnexpectedDisseminationMessage {
            role: validation_context.role,
        });
    }

    let dissemination = EnvelopeDissemination::from_ssz_bytes(
        validation_context.signed_ssv_message.ssv_message().data(),
    )
    .map_err(ValidationFailure::UndecodableMessageData)?;
    let slot = dissemination.slot;

    validate_role_for_fork(slot, &validation_context)?;

    // Rule: exactly one signer.
    let signers = validation_context.signed_ssv_message.operator_ids();
    if signers.len() != 1 {
        return Err(ValidationFailure::DisseminationOneSigner);
    }
    let signer = signers[0];

    // Rule: full data rides only consensus proposals.
    if !validation_context.signed_ssv_message.full_data().is_empty() {
        return Err(ValidationFailure::FullDataNotInConsensusMessage);
    }

    // Rule: `EnvelopeProposer` is a monotonic-slot role, so a signer that already advanced to
    // a later slot must not disseminate for an earlier one; the mirror of the partial-signature
    // path's guard, since both message classes advance the same shared `max_slot`.
    let max_slot = duty_state.get_or_create_operator(&signer).max_slot();
    if max_slot.as_u64() != 0 && max_slot > slot {
        return Err(ValidationFailure::SlotAlreadyAdvanced {
            got: slot.as_u64(),
            want: max_slot.as_u64(),
        });
    }

    // Rule: the validator must be the assigned proposer at the slot (Ignore when the epoch's
    // duties are not yet known locally).
    let is_randao_msg = false; // a dissemination carries no RANDAO signature
    validate_beacon_duty(
        &validation_context,
        slot,
        is_randao_msg,
        duty_provider.clone(),
    )?;

    // Rule: first-valid dedup per (`MessageId`, slot), signer-independent. Ignore-class.
    if duty_state.is_dissemination_recorded(slot) {
        return Err(ValidationFailure::RelayedDuplicateMessage {
            got: format!("envelope dissemination for slot {slot}"),
        });
    }

    // Rule: no earliness allowance, 3-slot lateness TTL (role-keyed).
    validate_slot_time(slot, &validation_context)?;

    // Rule: per-epoch duty limit (role-keyed, `SLOTS_PER_EPOCH`). Ignore-class.
    let operator_state = duty_state.get_or_create_operator(&signer);
    validate_duty_count(&validation_context, slot, operator_state, duty_provider)?;

    verify_single_signer(&validation_context, signer)?;

    // Record only after every other rule passed, so a rejected message cannot consume the
    // slot's single dissemination budget.
    duty_state.record_dissemination(slot, &signer);

    Ok(ValidatedSSVMessage::EnvelopeDissemination(dissemination))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use duties_tracker::DutyAssignment;
    use fork::Fork;
    use openssl::{
        hash::MessageDigest,
        pkey::{PKey, Private, Public},
        rsa::Rsa,
        sign::Signer,
    };
    use slot_clock::ManualSlotClock;
    use ssv_types::{
        OperatorId, VariableList,
        message::{MsgType, SSVMessage, SignedSSVMessage},
    };
    use ssz::Encode;
    use types::Slot;

    use super::*;
    use crate::{
        MessageAcceptance, ValidationContext,
        tests::{
            MockDutiesProvider, assert_validation_error, create_message_id_for_test,
            four_node_committee_and_keypair, generate_fork_schedule, generate_test_key_pair,
            spec_with_gloas,
        },
    };

    const SLOTS_PER_EPOCH_TEST: u64 = 32;
    /// Message slot used by most tests; the clock genesis sits one slot before it.
    const TEST_SLOT: u64 = 1;

    /// Builds a signed dissemination message for `role`'s message ID at `slot`, signed by
    /// each of `signers` with `private_key`, carrying `full_data`.
    fn create_signed_dissemination_with(
        role: Role,
        signers: Vec<OperatorId>,
        private_key: &Rsa<Private>,
        slot: Slot,
        full_data: Vec<u8>,
    ) -> SignedSSVMessage {
        let dissemination = EnvelopeDissemination {
            slot,
            envelope: VariableList::new(vec![0xAA; 64]).unwrap(),
        };
        let ssv_msg = SSVMessage::new(
            MsgType::SSVEnvelopeDisseminationMsgType,
            create_message_id_for_test(role),
            dissemination.as_ssz_bytes(),
        )
        .unwrap();

        let p_key = PKey::from_rsa(private_key.clone()).unwrap();
        let mut signer = Signer::new(MessageDigest::sha256(), &p_key).unwrap();
        signer.update(&ssv_msg.as_ssz_bytes()).unwrap();
        let signature: [u8; 256] = signer.sign_to_vec().unwrap().try_into().unwrap();

        let signatures = vec![signature; signers.len()];
        SignedSSVMessage::new(signatures, signers, ssv_msg, full_data).unwrap()
    }

    /// Single-signer dissemination at `TEST_SLOT` with no full data.
    fn create_signed_dissemination(
        role: Role,
        signer_id: OperatorId,
        private_key: &Rsa<Private>,
    ) -> SignedSSVMessage {
        create_signed_dissemination_with(
            role,
            vec![signer_id],
            private_key,
            Slot::new(TEST_SLOT),
            vec![],
        )
    }

    /// Context whose clock genesis sits one slot before `TEST_SLOT`: `slots_since_genesis: 1`
    /// receives the message exactly at its slot start (on time), larger values push it late.
    fn create_dissemination_context<'a>(
        signed_msg: &'a SignedSSVMessage,
        committee_info: &'a crate::CommitteeInfo,
        role: Role,
        operator_pub_keys: &'a HashMap<OperatorId, Rsa<Public>>,
        slots_since_genesis: u64,
        gloas_epoch: Option<u64>,
    ) -> ValidationContext<'a, ManualSlotClock> {
        let now = SystemTime::now();
        let slot_clock = ManualSlotClock::new(
            Slot::new(0),
            now.duration_since(UNIX_EPOCH).unwrap(),
            Duration::from_secs(12),
        );

        ValidationContext {
            signed_ssv_message: signed_msg,
            committee_info,
            role,
            received_at: now + Duration::from_secs(12 * slots_since_genesis),
            slots_per_epoch: SLOTS_PER_EPOCH_TEST,
            epochs_per_sync_committee_period: 256,
            sync_committee_size: 512,
            slot_clock,
            operator_pub_keys,
            fork_schedule: generate_fork_schedule(Fork::Boole),
            spec: spec_with_gloas(gloas_epoch),
        }
    }

    #[test]
    fn valid_dissemination_accepted() {
        let (committee_info, private_key, map) = four_node_committee_and_keypair();
        let signed_msg =
            create_signed_dissemination(Role::EnvelopeProposer, OperatorId(1), &private_key);
        let ctx = create_dissemination_context(
            &signed_msg,
            &committee_info,
            Role::EnvelopeProposer,
            &map,
            1,
            Some(0),
        );

        let result = validate_envelope_dissemination(
            ctx,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider::default()),
        );

        match result {
            Ok(ValidatedSSVMessage::EnvelopeDissemination(d)) => {
                assert_eq!(d.slot, Slot::new(TEST_SLOT));
            }
            other => panic!("expected accepted dissemination, got {other:?}"),
        }
    }

    #[test]
    fn non_envelope_role_rejected() {
        let (committee_info, private_key, map) = four_node_committee_and_keypair();
        let signed_msg = create_signed_dissemination(Role::Proposer, OperatorId(1), &private_key);
        let ctx = create_dissemination_context(
            &signed_msg,
            &committee_info,
            Role::Proposer,
            &map,
            1,
            Some(0),
        );

        let result = validate_envelope_dissemination(
            ctx,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider::default()),
        );

        match &result {
            Err(failure @ ValidationFailure::UnexpectedDisseminationMessage { .. }) => {
                assert_eq!(
                    MessageAcceptance::from(failure),
                    MessageAcceptance::Reject,
                    "dissemination on a foreign role must be Reject"
                );
            }
            other => panic!("expected UnexpectedDisseminationMessage, got {other:?}"),
        }
    }

    #[test]
    fn second_dissemination_for_slot_ignored_regardless_of_signer() {
        let (committee_info, private_key, mut map) = four_node_committee_and_keypair();
        // A second operator with its own key, so the dedup is proven signer-independent.
        let (private_key_2, public_key_2) = generate_test_key_pair();
        map.insert(OperatorId(2), public_key_2);
        let mut duty_state = DutyState::new(64);

        let first =
            create_signed_dissemination(Role::EnvelopeProposer, OperatorId(1), &private_key);
        let ctx = create_dissemination_context(
            &first,
            &committee_info,
            Role::EnvelopeProposer,
            &map,
            1,
            Some(0),
        );
        validate_envelope_dissemination(
            ctx,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        )
        .expect("first dissemination must be accepted");

        let second =
            create_signed_dissemination(Role::EnvelopeProposer, OperatorId(2), &private_key_2);
        let ctx = create_dissemination_context(
            &second,
            &committee_info,
            Role::EnvelopeProposer,
            &map,
            1,
            Some(0),
        );
        let result = validate_envelope_dissemination(
            ctx,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );

        match &result {
            Err(failure @ ValidationFailure::RelayedDuplicateMessage { .. }) => {
                assert_eq!(
                    MessageAcceptance::from(failure),
                    MessageAcceptance::Ignore,
                    "a further dissemination for a recorded slot must be Ignore, not Reject"
                );
            }
            other => panic!("expected RelayedDuplicateMessage, got {other:?}"),
        }
    }

    #[test]
    fn two_signers_rejected() {
        let (committee_info, private_key, map) = four_node_committee_and_keypair();
        // The one-signer rule fires before RSA verify, so the second signature's validity
        // is irrelevant.
        let signed_msg = create_signed_dissemination_with(
            Role::EnvelopeProposer,
            vec![OperatorId(1), OperatorId(2)],
            &private_key,
            Slot::new(TEST_SLOT),
            vec![],
        );

        let ctx = create_dissemination_context(
            &signed_msg,
            &committee_info,
            Role::EnvelopeProposer,
            &map,
            1,
            Some(0),
        );
        let result = validate_envelope_dissemination(
            ctx,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider::default()),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::DisseminationOneSigner),
            "DisseminationOneSigner",
        );
    }

    #[test]
    fn dissemination_below_advanced_slot_ignored() {
        let (committee_info, private_key, map) = four_node_committee_and_keypair();
        let mut duty_state = DutyState::new(64);

        // A dissemination at slot 2 advances the signer's max_slot.
        let later = create_signed_dissemination_with(
            Role::EnvelopeProposer,
            vec![OperatorId(1)],
            &private_key,
            Slot::new(TEST_SLOT + 1),
            vec![],
        );
        let ctx = create_dissemination_context(
            &later,
            &committee_info,
            Role::EnvelopeProposer,
            &map,
            2,
            Some(0),
        );
        validate_envelope_dissemination(
            ctx,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        )
        .expect("dissemination at the later slot must be accepted");

        // The same signer's dissemination for the earlier slot is now stale.
        let earlier =
            create_signed_dissemination(Role::EnvelopeProposer, OperatorId(1), &private_key);
        let ctx = create_dissemination_context(
            &earlier,
            &committee_info,
            Role::EnvelopeProposer,
            &map,
            2,
            Some(0),
        );
        let result = validate_envelope_dissemination(
            ctx,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::SlotAlreadyAdvanced { .. }),
            "SlotAlreadyAdvanced (monotonic-slot role)",
        );
    }

    #[test]
    fn beyond_short_ttl_rejected() {
        let (committee_info, private_key, map) = four_node_committee_and_keypair();
        let signed_msg =
            create_signed_dissemination(Role::EnvelopeProposer, OperatorId(1), &private_key);
        // Received 4 slots after the message slot's start, past the role's short TTL
        // (1 + LATE_SLOT_ALLOWANCE = 3 slots).
        let ctx = create_dissemination_context(
            &signed_msg,
            &committee_info,
            Role::EnvelopeProposer,
            &map,
            5,
            Some(0),
        );

        let result = validate_envelope_dissemination(
            ctx,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider::default()),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::LateSlotMessage { .. }),
            "LateSlotMessage (dissemination past short TTL)",
        );
    }

    #[test]
    fn rejected_before_gloas() {
        let (committee_info, private_key, map) = four_node_committee_and_keypair();
        let signed_msg =
            create_signed_dissemination(Role::EnvelopeProposer, OperatorId(1), &private_key);
        let ctx = create_dissemination_context(
            &signed_msg,
            &committee_info,
            Role::EnvelopeProposer,
            &map,
            1,
            None,
        );

        let result = validate_envelope_dissemination(
            ctx,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider::default()),
        );

        assert_validation_error(
            result,
            |failure| {
                matches!(
                    failure,
                    ValidationFailure::RoleNotActiveBeforeEthFork { .. }
                )
            },
            "RoleNotActiveBeforeEthFork (dissemination before Gloas)",
        );
    }

    #[test]
    fn failing_message_does_not_consume_the_slot_budget() {
        let (committee_info, private_key, map) = four_node_committee_and_keypair();
        let mut duty_state = DutyState::new(64);

        // First message fails the proposer-assignment check (Ignore), so it must not record.
        let first =
            create_signed_dissemination(Role::EnvelopeProposer, OperatorId(1), &private_key);
        let ctx = create_dissemination_context(
            &first,
            &committee_info,
            Role::EnvelopeProposer,
            &map,
            1,
            Some(0),
        );
        let result = validate_envelope_dissemination(
            ctx,
            &mut duty_state,
            Arc::new(MockDutiesProvider {
                proposer_assignment: DutyAssignment::NotAssigned,
                ..Default::default()
            }),
        );
        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::NoDuty),
            "NoDuty (unassigned proposer)",
        );

        // A valid dissemination for the same slot must still be accepted afterwards.
        let second =
            create_signed_dissemination(Role::EnvelopeProposer, OperatorId(1), &private_key);
        let ctx = create_dissemination_context(
            &second,
            &committee_info,
            Role::EnvelopeProposer,
            &map,
            1,
            Some(0),
        );
        validate_envelope_dissemination(
            ctx,
            &mut duty_state,
            Arc::new(MockDutiesProvider::default()),
        )
        .expect("a rejected message must not consume the slot's dissemination budget");
    }

    #[test]
    fn full_data_rejected() {
        let (committee_info, private_key, map) = four_node_committee_and_keypair();
        let signed_msg = create_signed_dissemination_with(
            Role::EnvelopeProposer,
            vec![OperatorId(1)],
            &private_key,
            Slot::new(TEST_SLOT),
            vec![0xBB; 8],
        );

        let ctx = create_dissemination_context(
            &signed_msg,
            &committee_info,
            Role::EnvelopeProposer,
            &map,
            1,
            Some(0),
        );
        let result = validate_envelope_dissemination(
            ctx,
            &mut DutyState::new(64),
            Arc::new(MockDutiesProvider::default()),
        );

        assert_validation_error(
            result,
            |failure| matches!(failure, ValidationFailure::FullDataNotInConsensusMessage),
            "FullDataNotInConsensusMessage",
        );
    }
}

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use database::OwnOperatorId;
use message_validator::ValidatedSSVMessage;
use ssv_types::{Slot, message::SignedSSVMessage, msgid::Role};
use tokio::sync::watch;
use tracing::{error, info};

/// Extract slot from validated SSV message
///
/// Returns the slot from either:
/// - QBFT message: extracted from `height` field
/// - PartialSignatureMessages: extracted from `slot` field
fn extract_message_slot(validated_message: &ValidatedSSVMessage) -> Slot {
    match validated_message {
        ValidatedSSVMessage::QbftMessage(msg) => Slot::new(msg.height),
        ValidatedSSVMessage::PartialSignatureMessages(msg) => msg.slot,
        ValidatedSSVMessage::EnvelopeDissemination(msg) => msg.slot,
    }
}

pub struct OperatorDoppelgangerService {
    /// Our operator ID to watch for (wraps database watch)
    own_operator_id: OwnOperatorId,
    /// Whether actively monitoring for doppelgängers (AtomicBool for lock-free access)
    is_monitoring: AtomicBool,
    /// Watch sender for signaling twin detection (allows immediate return from monitoring)
    twin_detected_tx: watch::Sender<bool>,
    /// Watch receiver for checking twin detection status
    twin_detected_rx: watch::Receiver<bool>,
    /// The slot at which this service started (used to filter our own old messages)
    startup_slot: Slot,
    /// Number of slots per epoch (for calculating monitoring duration)
    slots_per_epoch: u64,
    /// Duration of a slot (for calculating monitoring duration)
    slot_duration: Duration,
}

impl OperatorDoppelgangerService {
    /// Create a new operator doppelgänger service
    ///
    /// ## Parameters
    /// * `startup_slot` - The current slot at service creation (used to filter our own old
    ///   messages)
    pub fn new(
        own_operator_id: OwnOperatorId,
        startup_slot: Slot,
        slots_per_epoch: u64,
        slot_duration: Duration,
    ) -> Self {
        let (twin_detected_tx, twin_detected_rx) = watch::channel(false);
        Self {
            own_operator_id,
            is_monitoring: AtomicBool::new(true), // Start in monitoring mode
            twin_detected_tx,
            twin_detected_rx,
            startup_slot,
            slots_per_epoch,
            slot_duration,
        }
    }

    /// Block and monitor for doppelgängers during the configured wait period
    ///
    /// This method blocks execution for the monitoring duration while incoming messages
    /// are checked for twins. Returns immediately if a twin is detected, otherwise waits
    /// for the full monitoring period.
    ///
    /// ## Parameters
    /// * `wait_epochs` - Number of epochs to monitor for twins
    ///
    /// ## Returns
    /// * `Ok(())` - Monitoring period completed without detecting a twin
    /// * `Err(String)` - Twin detected during monitoring with error message
    ///
    /// The caller should handle the error by shutting down the client gracefully.
    pub async fn monitor_blocking(&self, wait_epochs: u64) -> Result<(), String> {
        let monitoring_slots = wait_epochs * self.slots_per_epoch;
        let monitoring_duration =
            Duration::from_secs(monitoring_slots * self.slot_duration.as_secs());

        info!(
            startup_slot = self.startup_slot.as_u64(),
            monitoring_epochs = wait_epochs,
            monitoring_secs = monitoring_duration.as_secs(),
            "Operator doppelgänger: starting slot-based monitoring (blocking)"
        );

        // Clone the receiver for waiting
        let mut twin_rx = self.twin_detected_rx.clone();

        // Wait for either timeout or twin detection
        tokio::select! {
            _ = tokio::time::sleep(monitoring_duration) => {
                // Timeout reached - monitoring complete
                info!("Operator doppelgänger: monitoring period complete - no twin detected");
                self.is_monitoring.store(false, Ordering::Release);
                Ok(())
            }
            _ = twin_rx.changed() => {
                // Twin detected signal received
                if *twin_rx.borrow() {
                    Err("Operator doppelgänger detected during monitoring".to_string())
                } else {
                    // False alarm, continue waiting
                    // This shouldn't happen in practice, but handle it gracefully
                    Ok(())
                }
            }
        }
    }

    /// Check if a message indicates a potential doppelgänger (detection logic only)
    ///
    /// Returns `true` if a twin operator is detected, `false` otherwise.
    /// This method performs pure detection logic without side effects (except logging).
    ///
    /// ## Slot-Based Detection
    ///
    /// Uses slot comparison to distinguish our old messages from twin messages:
    /// - Messages with `slot <= startup_slot`: Ignored (our own old messages)
    /// - Messages with `slot > startup_slot`: Twin detected (another instance running)
    ///
    /// ## Why This Works
    ///
    /// During the entire monitoring period, we block ALL outgoing messages. This means:
    /// 1. Any message for `slot > startup_slot` MUST be from a twin (we didn't send it)
    /// 2. Messages for `startup_slot` are ignored (could be ours from before restart)
    /// 3. No race conditions possible (we never compete with twins)
    ///
    /// ## Edge Cases Handled
    ///
    /// - **Restart in same slot**: Our old messages for that slot are ignored
    /// - **Network delays**: Slot comparison is delay-independent
    /// - **Clock skew**: Minor clock differences (1-2 slots) are tolerable
    pub fn is_doppelganger(
        &self,
        signed_message: &SignedSSVMessage,
        validated_message: &ValidatedSSVMessage,
    ) -> bool {
        // Fast path: atomic load for monitoring state (lock-free)
        if !self.is_monitoring.load(Ordering::Relaxed) {
            return false;
        }

        // Role-aware freshness exemption: a `ProposerPreferences` envelope slot is the duty's
        // FUTURE `proposal_slot`, not the message's emission time, so `msg_slot > startup_slot` is
        // not evidence of a live twin. Replaying a valid packet signed by a prior process (for a
        // proposal_slot still ahead of this instance's startup) would otherwise false-positive and
        // abort a legitimate replacement. The wire carries no authenticated emission time, so
        // slot-based freshness cannot distinguish an old packet from a twin here; exclude the role.
        if signed_message.ssv_message().msg_id().role() == Some(Role::ProposerPreferences) {
            return false;
        }

        // Extract slot from validated message (no decoding needed)
        let msg_slot = extract_message_slot(validated_message);

        // Only detect twins for messages AFTER our startup
        // Messages at or before startup_slot are ignored (our own old messages)
        if msg_slot <= self.startup_slot {
            return false;
        }

        // Get operator ID - return early if not yet available (still syncing)
        let Some(own_operator_id) = self.own_operator_id.get() else {
            return false;
        };

        // Check if this is a single-signer message with our operator ID
        let operator_ids = signed_message.operator_ids();
        if operator_ids.len() != 1 {
            // Not a single-signer message (could be aggregate/decided)
            return false;
        }

        let signer = operator_ids[0];
        if signer != own_operator_id {
            // Not signed by us
            return false;
        }

        // Twin detected: single-signer message with our operator ID for slot > startup_slot
        let msg_id = signed_message.ssv_message().msg_id();

        // Extract logging context from validated message
        match validated_message {
            ValidatedSSVMessage::QbftMessage(msg) => {
                error!(
                    operator_id = *own_operator_id,
                    duty_executor = ?msg_id.duty_executor(),
                    msg_slot = msg_slot.as_u64(),
                    startup_slot = self.startup_slot.as_u64(),
                    height = msg.height,
                    round = msg.round,
                    qbft_type = ?msg.qbft_message_type,
                    "OPERATOR DOPPELGÄNGER DETECTED: Received QBFT message signed with our operator ID for slot after startup. \
                     Another instance of this operator is running. Shutting down to prevent equivocation."
                );
            }
            ValidatedSSVMessage::PartialSignatureMessages(msg) => {
                error!(
                    operator_id = *own_operator_id,
                    duty_executor = ?msg_id.duty_executor(),
                    msg_slot = msg_slot.as_u64(),
                    startup_slot = self.startup_slot.as_u64(),
                    partial_sig_kind = ?msg.kind,
                    num_messages = msg.messages.len(),
                    "OPERATOR DOPPELGÄNGER DETECTED: Received partial signature message signed with our operator ID for slot after startup. \
                     Another instance of this operator is running. Shutting down to prevent equivocation."
                );
            }
            ValidatedSSVMessage::EnvelopeDissemination(_) => {
                error!(
                    operator_id = *own_operator_id,
                    duty_executor = ?msg_id.duty_executor(),
                    msg_slot = msg_slot.as_u64(),
                    startup_slot = self.startup_slot.as_u64(),
                    "OPERATOR DOPPELGÄNGER DETECTED: Received envelope dissemination signed with our operator ID for slot after startup. \
                     Another instance of this operator is running. Shutting down to prevent equivocation."
                );
            }
        }

        true
    }

    /// Check if a message indicates a potential doppelgänger
    ///
    /// Checks the message and signals twin detection if a twin is detected,
    /// allowing `monitor_blocking()` to return immediately.
    pub fn check_message(
        &self,
        signed_message: &SignedSSVMessage,
        validated_message: &ValidatedSSVMessage,
    ) {
        if self.is_doppelganger(signed_message, validated_message) {
            // Signal twin detection (this will wake up monitor_blocking)
            let _ = self.twin_detected_tx.send(true);
        }
    }

    /// Check if actively monitoring for doppelgängers
    ///
    /// Returns `true` during the monitoring period (from service creation until
    /// monitoring duration expires). Returns `false` after monitoring completes.
    ///
    /// Used to:
    /// 1. Block outgoing messages during monitoring (prevent competition with twins)
    /// 2. Enable incoming message detection during monitoring
    pub fn is_monitoring(&self) -> bool {
        self.is_monitoring.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bls::{PublicKeyBytes, Signature};
    use database::OwnOperatorId;
    use ssv_types::{
        CommitteeId, OperatorId, RSA_SIGNATURE_SIZE, ValidatorIndex, VariableList,
        consensus::{QbftMessage, QbftMessageType},
        domain_type::DomainType,
        message::{MsgType, SSVMessage, SignedSSVMessage},
        msgid::{DutyExecutor, MessageId, Role},
        partial_sig::{PartialSignatureKind, PartialSignatureMessage, PartialSignatureMessages},
    };
    use types::Hash256;

    use super::*;

    fn create_service_with_slot(startup_slot: Slot) -> OperatorDoppelgangerService {
        let own_operator_id = OwnOperatorId::from(OperatorId(1));
        let slots_per_epoch = 1;
        let slot_duration = Duration::from_secs(12);

        OperatorDoppelgangerService::new(
            own_operator_id,
            startup_slot,
            slots_per_epoch,
            slot_duration,
        )
    }

    /// Helper to create test messages for doppelgänger detection
    ///
    /// # Arguments
    /// * `committee_id` - The committee identifier
    /// * `operator_ids` - Vector of operator IDs (single for non-aggregated, multiple for
    ///   aggregated)
    /// * `height` - QBFT consensus height
    /// * `round` - QBFT consensus round
    fn create_test_message(
        committee_id: CommitteeId,
        operator_ids: Vec<OperatorId>,
        height: u64,
        round: u64,
    ) -> (SignedSSVMessage, ValidatedSSVMessage) {
        // Create MessageId for committee messages
        let message_id = MessageId::new(
            &DomainType([0; 4]),
            Role::Committee,
            &DutyExecutor::Committee(committee_id),
        );

        // Create QbftMessage
        let qbft_message = QbftMessage {
            qbft_message_type: QbftMessageType::Prepare,
            height,
            round,
            identifier: VariableList::new(message_id.as_ref().to_vec()).unwrap(),
            root: Hash256::from([0u8; 32]),
            data_round: 0,
            round_change_justification: vec![].try_into().unwrap(),
            prepare_justification: vec![].try_into().unwrap(),
        };

        // Create SSVMessage with serialized QbftMessage
        // Note: Since ethereum_ssz::Encode isn't directly accessible in this test module,
        // we use a minimal test payload. This is acceptable since we're testing the
        // doppelgänger detection logic, not QBFT message serialization.
        let qbft_bytes = vec![0u8; 100];
        let ssv_message = SSVMessage::new(MsgType::SSVConsensusMsgType, message_id, qbft_bytes)
            .expect("should create SSVMessage");

        // Create signatures (one per operator)
        let signatures: Vec<[u8; RSA_SIGNATURE_SIZE]> = operator_ids
            .iter()
            .map(|_| [0u8; RSA_SIGNATURE_SIZE])
            .collect();

        // Create SignedSSVMessage
        let signed_message = SignedSSVMessage::new(
            signatures,
            operator_ids,
            ssv_message,
            vec![], // empty full_data for non-proposal messages
        )
        .expect("should create SignedSSVMessage");

        // Wrap in ValidatedSSVMessage
        let validated_message = ValidatedSSVMessage::QbftMessage(qbft_message);

        (signed_message, validated_message)
    }

    /// Helper to build a single-signer validator-scoped `PartialSignatureMessages` for
    /// doppelgänger detection.
    ///
    /// The `MessageId` is validator-scoped (`DutyExecutor::Validator`) so
    /// `signed_message.ssv_message().msg_id().role()` reads `role`, which drives the role-aware
    /// freshness exemption in `is_doppelganger`. The envelope `slot` populates
    /// `PartialSignatureMessages::slot`, the value `extract_message_slot` reads.
    ///
    /// # Arguments
    /// * `role` - The role encoded into the `MessageId`
    /// * `kind` - The `PartialSignatureKind` carried by the envelope
    /// * `signer` - The single signing `OperatorId`
    /// * `slot` - The envelope slot (a `proposal_slot` for `ProposerPreferences`)
    fn create_partial_sig_message(
        role: Role,
        kind: PartialSignatureKind,
        signer: OperatorId,
        slot: Slot,
    ) -> (SignedSSVMessage, ValidatedSSVMessage) {
        // Validator-scoped MessageId so `msg_id().role()` returns `role`.
        let message_id = MessageId::new(
            &DomainType([0; 4]),
            role,
            &DutyExecutor::Validator(PublicKeyBytes::empty()),
        );

        let partial_sig_messages = PartialSignatureMessages {
            kind,
            slot,
            messages: VariableList::new(vec![PartialSignatureMessage {
                partial_signature: Signature::empty(),
                signing_root: Hash256::from([0u8; 32]),
                signer,
                validator_index: ValidatorIndex(0),
            }])
            .unwrap(),
        };

        // Payload bytes are irrelevant to detection: `is_doppelganger` reads the pre-decoded
        // `ValidatedSSVMessage`, so a minimal placeholder mirrors `create_test_message`.
        let ssv_message = SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            message_id,
            vec![0u8; 100],
        )
        .expect("should create SSVMessage");

        let signed_message = SignedSSVMessage::new(
            vec![[0u8; RSA_SIGNATURE_SIZE]],
            vec![signer],
            ssv_message,
            vec![],
        )
        .expect("should create SignedSSVMessage");

        let validated_message = ValidatedSSVMessage::PartialSignatureMessages(partial_sig_messages);

        (signed_message, validated_message)
    }

    #[test]
    fn test_service_creation() {
        let service = create_service_with_slot(Slot::new(100));

        // Start in monitoring mode
        assert!(
            service.is_monitoring(),
            "Should start monitoring immediately"
        );
        assert_eq!(
            service.startup_slot,
            Slot::new(100),
            "Startup slot should be set"
        );
    }

    #[test]
    fn test_slot_extraction_from_qbft() {
        // Create a QBFT message with height 12345
        let committee_id = CommitteeId([1u8; 32]);
        let (_signed_message, validated_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 12345, 1);

        // Extract slot should return the QBFT height
        let slot = extract_message_slot(&validated_message);
        assert_eq!(
            slot,
            Slot::new(12345),
            "Should extract slot from QBFT height"
        );
    }

    // High-value tests for slot-based detection

    #[test]
    fn test_twin_detected_slot_after_startup() {
        // Create service with startup_slot = 100
        let service = create_service_with_slot(Slot::new(100));
        let committee_id = CommitteeId([1u8; 32]);

        // Create a message for slot 101 (after startup) with our operator ID (1)
        let (signed_message, validated_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 101, 0);

        // This should detect a twin (message slot > startup_slot)
        let result = service.is_doppelganger(&signed_message, &validated_message);
        assert!(result, "Message for slot after startup should detect twin");
    }

    #[test]
    fn test_no_twin_proposer_preferences_future_proposal_slot() {
        // A `ProposerPreferences` envelope slot is the duty's FUTURE `proposal_slot`, not the
        // message's emission time. A valid packet signed by a PRIOR process (for a `proposal_slot`
        // still ahead of this instance's `startup_slot`) can be replayed on the wire; without the
        // role-aware exemption its `slot > startup_slot` would false-positive as a live twin and
        // abort a legitimate replacement.

        // Arrange: startup_slot = 100; a single-signer role-8 packet from our own `OperatorId(1)`
        // whose envelope slot 120 is a future proposal_slot (> startup).
        let service = create_service_with_slot(Slot::new(100));
        let (signed_message, validated_message) = create_partial_sig_message(
            Role::ProposerPreferences,
            PartialSignatureKind::ProposerPreferences,
            OperatorId(1),
            Slot::new(120),
        );

        // Act
        let result = service.is_doppelganger(&signed_message, &validated_message);

        // Assert: the future proposal_slot must NOT be treated as twin evidence.
        assert!(
            !result,
            "ProposerPreferences future proposal_slot should NOT detect twin (replayed prior-process packet)"
        );
    }

    #[test]
    fn test_twin_detected_non_exempt_partial_sig_future_slot() {
        // Contrast to `test_no_twin_proposer_preferences_future_proposal_slot`: the freshness
        // exemption is specific to `Role::ProposerPreferences`, NOT a blanket skip of all
        // validator-scoped partial-signature messages. `Role::PTCAttester` is a non-exempt
        // partial-sig role whose envelope slot IS the emission slot, so a future-slot packet
        // signed with our own operator ID still indicates a live twin.

        // Arrange: startup_slot = 100; a single-signer `PTCAttester` packet from our own
        // `OperatorId(1)` whose envelope slot 101 is after startup.
        let service = create_service_with_slot(Slot::new(100));
        let (signed_message, validated_message) = create_partial_sig_message(
            Role::PTCAttester,
            PartialSignatureKind::PTCAttester,
            OperatorId(1),
            Slot::new(101),
        );

        // Act
        let result = service.is_doppelganger(&signed_message, &validated_message);

        // Assert: a non-exempt role for slot > startup_slot detects a twin.
        assert!(
            result,
            "Non-exempt PTCAttester message for slot after startup should detect twin"
        );
    }

    #[test]
    fn test_no_twin_slot_at_startup() {
        // Create service with startup_slot = 100
        let service = create_service_with_slot(Slot::new(100));
        let committee_id = CommitteeId([1u8; 32]);

        // Create a message for slot 100 (at startup) with our operator ID (1)
        let (signed_message, validated_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 100, 0);

        // This should NOT detect a twin (message slot <= startup_slot)
        let result = service.is_doppelganger(&signed_message, &validated_message);
        assert!(
            !result,
            "Message for startup slot should NOT detect twin (our own old message)"
        );
    }

    #[test]
    fn test_no_twin_slot_before_startup() {
        // Create service with startup_slot = 100
        let service = create_service_with_slot(Slot::new(100));
        let committee_id = CommitteeId([1u8; 32]);

        // Create a message for slot 99 (before startup) with our operator ID (1)
        let (signed_message, validated_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 99, 0);

        // This should NOT detect a twin (message slot < startup_slot)
        let result = service.is_doppelganger(&signed_message, &validated_message);
        assert!(
            !result,
            "Message for slot before startup should NOT detect twin (our own old message)"
        );
    }

    #[test]
    fn test_no_twin_multi_signer_aggregate_message() {
        let service = create_service_with_slot(Slot::new(100));
        let committee_id = CommitteeId([1u8; 32]);

        // Create a multi-signer aggregate message (includes our operator ID) for slot 101
        let (signed_message, validated_message) = create_test_message(
            committee_id,
            vec![OperatorId(1), OperatorId(2), OperatorId(3)],
            101,
            0,
        );

        // This should NOT detect a twin (aggregate message, not single-signer)
        let result = service.is_doppelganger(&signed_message, &validated_message);
        assert!(
            !result,
            "Multi-signer aggregate message should NOT detect twin"
        );
    }

    #[test]
    fn test_no_twin_different_operator_id() {
        let service = create_service_with_slot(Slot::new(100));
        let committee_id = CommitteeId([1u8; 32]);

        // Create a single-signer message from a different operator (2, not 1) for slot 101
        let (signed_message, validated_message) =
            create_test_message(committee_id, vec![OperatorId(2)], 101, 0);

        // This should NOT detect a twin (different operator)
        let result = service.is_doppelganger(&signed_message, &validated_message);
        assert!(
            !result,
            "Message from different operator should NOT detect twin"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_no_twin_after_monitoring_period_completes() {
        use std::sync::Arc;

        let service = Arc::new(create_service_with_slot(Slot::new(100)));
        let committee_id = CommitteeId([1u8; 32]);

        // Monitor blocking for 2 epochs
        let wait_epochs = 2;
        let monitoring_slots = wait_epochs * service.slots_per_epoch;
        let monitoring_duration =
            Duration::from_secs(monitoring_slots * service.slot_duration.as_secs());

        // Start monitoring in a background task (simulating the client's blocking call)
        let service_clone = Arc::clone(&service);
        let monitor_handle =
            tokio::spawn(async move { service_clone.monitor_blocking(wait_epochs).await });

        // Advance time past monitoring period
        tokio::time::advance(monitoring_duration).await;

        // Wait for monitoring to complete
        let result = monitor_handle.await.unwrap();
        assert!(result.is_ok(), "Monitoring should complete successfully");

        // After monitoring completes, is_monitoring should be false
        assert!(
            !service.is_monitoring(),
            "Monitoring should be complete after timer expires"
        );

        // Create a single-signer message with our operator ID for slot 101
        let (signed_message, validated_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 101, 0);

        // This should NOT detect a twin (monitoring period completed)
        let result = service.is_doppelganger(&signed_message, &validated_message);
        assert!(
            !result,
            "Message after monitoring period should NOT detect twin (monitoring complete)"
        );
    }
}

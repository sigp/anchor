use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use database::OwnOperatorId;
use futures::channel::mpsc;
use parking_lot::Mutex;
use ssv_types::{
    Slot, consensus::QbftMessage, message::SignedSSVMessage, partial_sig::PartialSignatureMessages,
};
use ssz::Decode;
use task_executor::{ShutdownReason, TaskExecutor};
use tracing::{error, info};

/// Extract slot from SSV message
///
/// Attempts to extract the slot from either:
/// 1. QBFT message (if provided) - extracts from `height` field
/// 2. PartialSignatureMessages (parses from message data) - extracts from `slot` field
///
/// Returns `None` if slot cannot be extracted (corrupted message).
fn extract_message_slot(
    signed_message: &SignedSSVMessage,
    qbft_message: Option<&QbftMessage>,
) -> Option<Slot> {
    // Try QBFT first (already parsed, most common case)
    if let Some(qbft) = qbft_message {
        return Some(Slot::new(qbft.height));
    }

    // Try PartialSignatureMessages (need to parse from SSZ)
    if let Ok(partial) =
        PartialSignatureMessages::from_ssz_bytes(signed_message.ssv_message().data())
    {
        return Some(partial.slot);
    }

    // Can't extract slot (corrupted/unknown message type)
    None
}

pub struct OperatorDoppelgangerService {
    /// Our operator ID to watch for (wraps database watch)
    own_operator_id: OwnOperatorId,
    /// Whether actively monitoring for doppelgängers (AtomicBool for lock-free access)
    is_monitoring: AtomicBool,
    /// The slot at which this service started (used to filter our own old messages)
    startup_slot: Slot,
    /// Number of slots per epoch (for calculating monitoring duration)
    slots_per_epoch: u64,
    /// Duration of a slot (for calculating monitoring duration)
    slot_duration: Duration,
    /// Shutdown sender (triggers fatal shutdown on twin detection)
    shutdown_sender: Mutex<mpsc::Sender<ShutdownReason>>,
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
        shutdown_sender: mpsc::Sender<ShutdownReason>,
    ) -> Self {
        Self {
            own_operator_id,
            is_monitoring: AtomicBool::new(true), // Start in monitoring mode
            startup_slot,
            slots_per_epoch,
            slot_duration,
            shutdown_sender: Mutex::new(shutdown_sender),
        }
    }

    /// Spawn a background task to end monitoring after the configured wait period
    ///
    /// Monitors the network for the specified number of epochs. During this period,
    /// all outgoing messages are blocked and incoming messages are checked for twins
    /// using slot-based detection (messages with slot > startup_slot from our operator).
    pub fn spawn_monitor_task(self: Arc<Self>, wait_epochs: u64, executor: &TaskExecutor) {
        let monitoring_slots = wait_epochs * self.slots_per_epoch;
        let monitoring_duration =
            Duration::from_secs(monitoring_slots * self.slot_duration.as_secs());

        executor.spawn_without_exit(
            async move {
                info!(
                    startup_slot = self.startup_slot.as_u64(),
                    monitoring_epochs = wait_epochs,
                    monitoring_secs = monitoring_duration.as_secs(),
                    "Operator doppelgänger: starting slot-based monitoring"
                );

                tokio::time::sleep(monitoring_duration).await;

                info!("Operator doppelgänger: monitoring period complete");
                self.is_monitoring.store(false, Ordering::Release);
            },
            "doppelganger-monitor",
        );
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
    /// - **Corrupted messages**: If slot can't be extracted, message is ignored
    pub fn is_doppelganger(
        &self,
        signed_message: &SignedSSVMessage,
        qbft_message: Option<&QbftMessage>,
    ) -> bool {
        // Fast path: atomic load for monitoring state (lock-free)
        if !self.is_monitoring.load(Ordering::Relaxed) {
            return false;
        }

        // Extract slot from message (QBFT height or PartialSignatureMessages slot)
        let Some(msg_slot) = extract_message_slot(signed_message, qbft_message) else {
            // Can't determine slot - ignore conservatively (prevents false positives)
            return false;
        };

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
        error!(
            operator_id = *own_operator_id,
            duty_executor = ?msg_id.duty_executor(),
            msg_slot = msg_slot.as_u64(),
            startup_slot = self.startup_slot.as_u64(),
            height = ?qbft_message.map(|m| m.height),
            round = ?qbft_message.map(|m| m.round),
            qbft_type = ?qbft_message.map(|m| m.qbft_message_type),
            "OPERATOR DOPPELGÄNGER DETECTED: Received message signed with our operator ID for slot after startup. \
             Another instance of this operator is running. Shutting down to prevent equivocation."
        );

        true
    }

    /// Check if a message indicates a potential doppelgänger
    ///
    /// Checks the message and triggers shutdown if a twin is detected
    pub fn check_message(
        &self,
        signed_message: &SignedSSVMessage,
        qbft_message: Option<&QbftMessage>,
    ) {
        if self.is_doppelganger(signed_message, qbft_message) {
            // Trigger shutdown
            let _ = self
                .shutdown_sender
                .lock()
                .try_send(ShutdownReason::Failure("Operator doppelgänger detected"));
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
    use std::{sync::Arc, time::Duration};

    use database::OwnOperatorId;
    use ssv_types::{
        CommitteeId, OperatorId, RSA_SIGNATURE_SIZE,
        consensus::{QbftMessage, QbftMessageType},
        domain_type::DomainType,
        message::{MsgType, SSVMessage, SignedSSVMessage},
        msgid::{DutyExecutor, MessageId, Role},
    };
    use task_executor::TaskExecutor;
    use types::Hash256;

    use super::*;

    /// Helper to create a TaskExecutor for testing
    fn create_test_executor() -> TaskExecutor {
        let handle = tokio::runtime::Handle::current();
        let (_signal, exit) = async_channel::bounded(1);
        let (shutdown_tx, _) = futures::channel::mpsc::channel(1);
        TaskExecutor::new(handle, exit, shutdown_tx, "doppelganger_test".into())
    }

    fn create_service_with_slot(startup_slot: Slot) -> OperatorDoppelgangerService {
        let own_operator_id = OwnOperatorId::from(OperatorId(1));
        let slots_per_epoch = 1;
        let slot_duration = Duration::from_secs(12);

        // Create a shutdown channel for testing
        let (shutdown_tx, _shutdown_rx) = mpsc::channel(1);

        OperatorDoppelgangerService::new(
            own_operator_id,
            startup_slot,
            slots_per_epoch,
            slot_duration,
            shutdown_tx,
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
    ) -> (SignedSSVMessage, QbftMessage) {
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
            identifier: message_id.as_ref().to_vec().into(),
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

        (signed_message, qbft_message)
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
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 12345, 1);

        // Extract slot should return the QBFT height
        let slot = extract_message_slot(&signed_message, Some(&qbft_message));
        assert_eq!(
            slot,
            Some(Slot::new(12345)),
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
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 101, 0);

        // This should detect a twin (message slot > startup_slot)
        let result = service.is_doppelganger(&signed_message, Some(&qbft_message));
        assert!(result, "Message for slot after startup should detect twin");
    }

    #[test]
    fn test_no_twin_slot_at_startup() {
        // Create service with startup_slot = 100
        let service = create_service_with_slot(Slot::new(100));
        let committee_id = CommitteeId([1u8; 32]);

        // Create a message for slot 100 (at startup) with our operator ID (1)
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 100, 0);

        // This should NOT detect a twin (message slot <= startup_slot)
        let result = service.is_doppelganger(&signed_message, Some(&qbft_message));
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
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 99, 0);

        // This should NOT detect a twin (message slot < startup_slot)
        let result = service.is_doppelganger(&signed_message, Some(&qbft_message));
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
        let (signed_message, qbft_message) = create_test_message(
            committee_id,
            vec![OperatorId(1), OperatorId(2), OperatorId(3)],
            101,
            0,
        );

        // This should NOT detect a twin (aggregate message, not single-signer)
        let result = service.is_doppelganger(&signed_message, Some(&qbft_message));
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
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(2)], 101, 0);

        // This should NOT detect a twin (different operator)
        let result = service.is_doppelganger(&signed_message, Some(&qbft_message));
        assert!(
            !result,
            "Message from different operator should NOT detect twin"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_no_twin_after_monitoring_period_timer() {
        let service = Arc::new(create_service_with_slot(Slot::new(100)));
        let committee_id = CommitteeId([1u8; 32]);
        let executor = create_test_executor();

        // Spawn the monitor task
        let wait_epochs = 2;
        service.clone().spawn_monitor_task(wait_epochs, &executor);

        // Give the spawned task a chance to start
        tokio::task::yield_now().await;

        // Calculate monitoring duration from service configuration
        let monitoring_slots = wait_epochs * service.slots_per_epoch;
        let monitoring_duration =
            Duration::from_secs(monitoring_slots * service.slot_duration.as_secs());

        // Advance time past monitoring period
        tokio::time::advance(monitoring_duration).await;
        tokio::task::yield_now().await;

        // Monitoring should be complete
        assert!(
            !service.is_monitoring(),
            "Monitoring should be complete after timer expires"
        );

        // Create a single-signer message with our operator ID for slot 101
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 101, 0);

        // This should NOT detect a twin (monitoring period completed)
        let result = service.is_doppelganger(&signed_message, Some(&qbft_message));
        assert!(
            !result,
            "Message after monitoring period should NOT detect twin (monitoring complete)"
        );
    }
}

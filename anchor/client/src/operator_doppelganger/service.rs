use std::{marker::PhantomData, sync::Arc, time::Duration};

use parking_lot::Mutex;
use slot_clock::SlotClock;
use ssv_types::{
    OperatorId, consensus::QbftMessage, message::SignedSSVMessage, msgid::DutyExecutor,
};
use task_executor::TaskExecutor;
use tokio::sync::watch;
use tracing::{debug, error, info};
use types::{Epoch, EthSpec};

#[cfg(test)]
use super::state::DoppelgangerMode;
use super::state::DoppelgangerState;

pub struct OperatorDoppelgangerService<E: EthSpec, S: SlotClock> {
    /// Our operator ID to watch for
    own_operator_id: OperatorId,
    /// Current state
    state: Arc<Mutex<DoppelgangerState>>,
    /// Slot clock for epoch tracking
    slot_clock: S,
    /// Epoch when monitoring period ends
    monitor_end_epoch: Epoch,
    /// Duration of a slot (for sleep intervals)
    slot_duration: Duration,
    /// Monitoring status broadcaster
    is_monitoring_tx: watch::Sender<bool>,
    /// Phantom data for EthSpec
    _phantom: PhantomData<E>,
}

impl<E: EthSpec, S: SlotClock> OperatorDoppelgangerService<E, S> {
    /// Create a new operator doppelgänger service
    ///
    /// Returns the service and a watch receiver that broadcasts monitoring status.
    /// The receiver will be `true` during monitoring mode and `false` after transitioning to active
    /// mode.
    pub fn new(
        own_operator_id: OperatorId,
        slot_clock: S,
        current_epoch: types::Epoch,
        wait_epochs: u64,
        fresh_k: u64,
        slot_duration: Duration,
    ) -> (Self, watch::Receiver<bool>) {
        let state = Arc::new(Mutex::new(DoppelgangerState::new(fresh_k)));

        // Create watch channel, starting in monitoring mode
        let (is_monitoring_tx, is_monitoring_rx) = watch::channel(true);

        info!(
            operator_id = *own_operator_id,
            current_epoch = current_epoch.as_u64(),
            wait_epochs,
            fresh_k,
            "Operator doppelgänger protection enabled, entering monitor mode"
        );

        let monitor_end_epoch = current_epoch + wait_epochs;

        let service = Self {
            own_operator_id,
            state,
            slot_clock,
            monitor_end_epoch,
            slot_duration,
            is_monitoring_tx,
            _phantom: PhantomData,
        };

        (service, is_monitoring_rx)
    }

    /// Spawn a background task to monitor epoch progression and transition to active mode
    ///
    /// The task checks the current epoch every slot and automatically calls
    /// `transition_to_active()` when the monitoring period ends.
    pub fn spawn_monitor_task(self: Arc<Self>, executor: &TaskExecutor)
    where
        S: 'static,
    {
        executor.spawn_without_exit(
            async move {
                loop {
                    // Check every slot
                    tokio::time::sleep(self.slot_duration).await;

                    if let Some(slot) = self.slot_clock.now() {
                        let current_epoch = slot.epoch(E::slots_per_epoch());
                        if current_epoch >= self.monitor_end_epoch {
                            self.transition_to_active();
                            break; // Done monitoring
                        }
                    }
                }
            },
            "doppelganger-monitor",
        );
    }

    /// Transition from monitor mode to active mode
    ///
    /// This should be called when the monitoring period ends (based on epoch progression).
    /// It updates the internal state and broadcasts the change to all watch receivers.
    ///
    /// Note: This is automatically called by `spawn_monitor_task()`. Made public for testing.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn transition_to_active(&self) {
        let mut state = self.state.lock();
        if state.is_monitoring() {
            state.set_active();
            info!(
                operator_id = *self.own_operator_id,
                "Operator doppelgänger: monitoring period ended, transitioning to active mode"
            );
            // Broadcast the transition - all receivers will see false (not monitoring)
            if let Err(e) = self.is_monitoring_tx.send(false) {
                error!(
                    error = ?e,
                    "Failed to broadcast monitoring transition"
                );
            }
        }
    }

    /// Check if a message indicates a potential doppelgänger
    ///
    /// Returns true if a twin is detected (should trigger shutdown)
    #[must_use]
    pub fn check_message(
        &self,
        signed_message: &SignedSSVMessage,
        qbft_message: &QbftMessage,
    ) -> bool {
        let mut state = self.state.lock();

        // Only check in monitor mode (background task handles transition)
        if !state.is_monitoring() {
            return false;
        }

        // Extract committee ID from message
        let committee_id = match signed_message.ssv_message().msg_id().duty_executor() {
            Some(DutyExecutor::Committee(committee_id)) => committee_id,
            _ => return false, // Not a committee message
        };

        // Check if this is a single-signer message with our operator ID
        let operator_ids = signed_message.operator_ids();
        if operator_ids.len() != 1 {
            // Not a single-signer message (could be aggregate/decided)
            return false;
        }

        let signer = operator_ids[0];
        if signer != self.own_operator_id {
            // Not signed by us
            return false;
        }

        // Check if the message is fresh (before updating our tracking)
        if !state.is_fresh(committee_id, qbft_message.height) {
            // Stale message, likely a replay - not evidence of a twin
            debug!(
                operator_id = *self.own_operator_id,
                committee = ?committee_id,
                height = qbft_message.height,
                "Received stale message with our operator ID (likely replay), ignoring"
            );
            return false;
        }

        // Update height tracking for this fresh message
        state.update_max_height(committee_id, qbft_message.height);

        // Fresh single-signer message with our operator ID = twin detected!
        error!(
            operator_id = *self.own_operator_id,
            committee = ?committee_id,
            height = qbft_message.height,
            round = qbft_message.round,
            message_type = ?qbft_message.qbft_message_type,
            "OPERATOR DOPPELGÄNGER DETECTED: Received fresh message signed with our operator ID. \
             Another instance of this operator is running. Shutting down to prevent equivocation."
        );

        true
    }

    /// Get the current mode
    #[cfg(test)]
    #[must_use]
    pub fn mode(&self) -> DoppelgangerMode {
        self.state.lock().mode()
    }

    /// Check if we're still in monitor mode
    #[cfg(test)]
    #[must_use]
    pub fn is_monitoring(&self) -> bool {
        self.state.lock().is_monitoring()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use slot_clock::TestingSlotClock;
    use ssv_types::{
        CommitteeId, RSA_SIGNATURE_SIZE,
        consensus::{QbftMessage, QbftMessageType},
        domain_type::DomainType,
        message::{MsgType, SSVMessage, SignedSSVMessage},
        msgid::{DutyExecutor, MessageId, Role},
    };
    use types::{Epoch, Hash256, MainnetEthSpec, Slot};

    use super::*;

    type E = MainnetEthSpec;

    fn create_service(
        current_epoch: Epoch,
        wait_epochs: u64,
        fresh_k: u64,
    ) -> OperatorDoppelgangerService<E, TestingSlotClock> {
        let own_operator_id = OperatorId(1);
        let genesis_slot = Slot::new(0);
        let genesis_duration = Duration::from_secs(0);
        let slot_duration = Duration::from_secs(12);

        let slot_clock = TestingSlotClock::new(genesis_slot, genesis_duration, slot_duration);

        // Set the clock to the start of current_epoch
        slot_clock.set_slot(current_epoch.start_slot(E::slots_per_epoch()).as_u64());

        let (service, _receiver) = OperatorDoppelgangerService::new(
            own_operator_id,
            slot_clock,
            current_epoch,
            wait_epochs,
            fresh_k,
            slot_duration,
        );
        service
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
            identifier: message_id.as_ref().to_vec().try_into().unwrap(),
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
        let service = create_service(Epoch::new(100), 2, 3);
        assert!(service.is_monitoring());
        assert_eq!(service.mode(), DoppelgangerMode::Monitor);
    }

    #[test]
    fn test_monitoring_state_persists() {
        let service = create_service(Epoch::new(100), 5, 10);

        // Advance clock within monitoring period
        service
            .slot_clock
            .set_slot(Epoch::new(103).start_slot(E::slots_per_epoch()).as_u64());

        // State hasn't updated yet (no message checked)
        assert!(service.is_monitoring());
    }

    #[test]
    fn test_different_fresh_k_values() {
        let service1 = create_service(Epoch::new(100), 2, 3);
        let service2 = create_service(Epoch::new(100), 2, 10);

        assert!(service1.is_monitoring());
        assert!(service2.is_monitoring());
    }

    // High-value tests for check_message functionality

    #[test]
    fn test_twin_detected_fresh_single_signer_with_our_operator_id() {
        let service = create_service(Epoch::new(100), 2, 3);
        let committee_id = CommitteeId([1u8; 32]);

        // Create a fresh single-signer message with our operator ID (1)
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 10, 0);

        // This should detect a twin (return true)
        let result = service.check_message(&signed_message, &qbft_message);
        assert!(
            result,
            "Fresh single-signer message with our operator ID should detect twin"
        );
    }

    #[test]
    fn test_no_twin_stale_message_beyond_fresh_k_window() {
        let service = create_service(Epoch::new(100), 2, 3);
        let committee_id = CommitteeId([1u8; 32]);

        // First, establish a recent max height by sending a fresh message
        let (signed_message1, qbft_message1) =
            create_test_message(committee_id, vec![OperatorId(1)], 20, 0);
        let _ = service.check_message(&signed_message1, &qbft_message1);

        // Now send a stale message (beyond fresh_k=3 window, so height < 20-3 = 17)
        let (signed_message2, qbft_message2) =
            create_test_message(committee_id, vec![OperatorId(1)], 15, 0);

        // This should NOT detect a twin (stale message, likely replay)
        let result = service.check_message(&signed_message2, &qbft_message2);
        assert!(
            !result,
            "Stale message beyond fresh_k window should NOT detect twin"
        );
    }

    #[test]
    fn test_no_twin_multi_signer_aggregate_message() {
        let service = create_service(Epoch::new(100), 2, 3);
        let committee_id = CommitteeId([1u8; 32]);

        // Create a multi-signer aggregate message (includes our operator ID)
        let (signed_message, qbft_message) = create_test_message(
            committee_id,
            vec![OperatorId(1), OperatorId(2), OperatorId(3)],
            10,
            0,
        );

        // This should NOT detect a twin (aggregate message)
        let result = service.check_message(&signed_message, &qbft_message);
        assert!(
            !result,
            "Multi-signer aggregate message should NOT detect twin"
        );
    }

    #[test]
    fn test_no_twin_different_operator_id() {
        let service = create_service(Epoch::new(100), 2, 3);
        let committee_id = CommitteeId([1u8; 32]);

        // Create a single-signer message from a different operator (2, not 1)
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(2)], 10, 0);

        // This should NOT detect a twin (different operator)
        let result = service.check_message(&signed_message, &qbft_message);
        assert!(
            !result,
            "Message from different operator should NOT detect twin"
        );
    }

    #[test]
    fn test_no_twin_after_monitoring_period_ends() {
        let service = create_service(Epoch::new(100), 2, 3);
        let committee_id = CommitteeId([1u8; 32]);

        // Advance clock beyond monitoring period (100 + 2 = 102)
        service
            .slot_clock
            .set_slot(Epoch::new(102).start_slot(E::slots_per_epoch()).as_u64());

        // Explicitly transition to active (simulating what background task does)
        service.transition_to_active();

        // Create a fresh single-signer message with our operator ID
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 10, 0);

        // This should NOT detect a twin (monitoring period ended)
        let result = service.check_message(&signed_message, &qbft_message);
        assert!(
            !result,
            "Message after monitoring period should NOT detect twin"
        );
    }

    #[test]
    fn test_freshness_window_boundary() {
        let service = create_service(Epoch::new(100), 2, 3);
        let committee_id = CommitteeId([1u8; 32]);

        // Establish max height at 20
        let (signed_message1, qbft_message1) =
            create_test_message(committee_id, vec![OperatorId(1)], 20, 0);
        let result1 = service.check_message(&signed_message1, &qbft_message1);
        assert!(result1, "Initial fresh message should detect twin");

        // Fresh range is [20 - 3, 20] = [17, 20]

        // Test at baseline (17) - should be fresh
        let (signed_message2, qbft_message2) =
            create_test_message(committee_id, vec![OperatorId(1)], 17, 0);
        let result2 = service.check_message(&signed_message2, &qbft_message2);
        assert!(
            result2,
            "Message at baseline (max_height - K) should detect twin"
        );

        // Test just below baseline (16) - should be stale
        let (signed_message3, qbft_message3) =
            create_test_message(committee_id, vec![OperatorId(1)], 16, 0);
        let result3 = service.check_message(&signed_message3, &qbft_message3);
        assert!(
            !result3,
            "Message below baseline should NOT detect twin (stale)"
        );

        // Test above max (21) - should be fresh
        let (signed_message4, qbft_message4) =
            create_test_message(committee_id, vec![OperatorId(1)], 21, 0);
        let result4 = service.check_message(&signed_message4, &qbft_message4);
        assert!(
            result4,
            "Message above max_height should detect twin (fresh)"
        );
    }

    #[test]
    fn test_independent_committee_height_tracking() {
        let service = create_service(Epoch::new(100), 2, 3);
        let committee_id1 = CommitteeId([1u8; 32]);
        let committee_id2 = CommitteeId([2u8; 32]);

        // Establish max height for committee1 at 20
        let (signed_message1, qbft_message1) =
            create_test_message(committee_id1, vec![OperatorId(1)], 20, 0);
        let result1 = service.check_message(&signed_message1, &qbft_message1);
        assert!(result1, "Committee1 initial message should detect twin");

        // Message at height 15 for committee1 should be stale (< 20-3=17)
        let (signed_message2, qbft_message2) =
            create_test_message(committee_id1, vec![OperatorId(1)], 15, 0);
        let result2 = service.check_message(&signed_message2, &qbft_message2);
        assert!(!result2, "Committee1 stale message should NOT detect twin");

        // But height 15 for committee2 should be fresh (no prior messages for committee2)
        let (signed_message3, qbft_message3) =
            create_test_message(committee_id2, vec![OperatorId(1)], 15, 0);
        let result3 = service.check_message(&signed_message3, &qbft_message3);
        assert!(
            result3,
            "Committee2 first message should detect twin (independent tracking)"
        );
    }

    #[test]
    fn test_first_message_for_committee_always_fresh() {
        let service = create_service(Epoch::new(100), 2, 3);
        let committee_id = CommitteeId([1u8; 32]);

        // First message for a committee is always considered fresh, regardless of height
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 5, 0);

        let result = service.check_message(&signed_message, &qbft_message);
        assert!(
            result,
            "First message for committee should always be fresh and detect twin"
        );
    }

    #[test]
    fn test_increasing_heights_all_fresh() {
        let service = create_service(Epoch::new(100), 2, 3);
        let committee_id = CommitteeId([1u8; 32]);

        // Simulate normal progression of increasing heights - all should be fresh
        for height in 10..15 {
            let (signed_message, qbft_message) =
                create_test_message(committee_id, vec![OperatorId(1)], height, 0);
            let result = service.check_message(&signed_message, &qbft_message);
            assert!(
                result,
                "Increasing height {} should be fresh and detect twin",
                height
            );
        }
    }

    #[test]
    fn test_max_height_updates_correctly() {
        let service = create_service(Epoch::new(100), 2, 3);
        let committee_id = CommitteeId([1u8; 32]);

        // Send height 10
        let (signed_message1, qbft_message1) =
            create_test_message(committee_id, vec![OperatorId(1)], 10, 0);
        let _ = service.check_message(&signed_message1, &qbft_message1);

        // Send height 15 (new max)
        let (signed_message2, qbft_message2) =
            create_test_message(committee_id, vec![OperatorId(1)], 15, 0);
        let result2 = service.check_message(&signed_message2, &qbft_message2);
        assert!(result2, "New max height should be fresh");

        // Now height 11 should be stale (< 15-3=12)
        let (signed_message3, qbft_message3) =
            create_test_message(committee_id, vec![OperatorId(1)], 11, 0);
        let result3 = service.check_message(&signed_message3, &qbft_message3);
        assert!(
            !result3,
            "Height 11 should now be stale after max updated to 15"
        );
    }
}

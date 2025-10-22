use std::{marker::PhantomData, sync::Arc, time::Duration};

use database::OwnOperatorId;
use futures::channel::mpsc;
use parking_lot::Mutex;
use slot_clock::SlotClock;
use ssv_types::{consensus::QbftMessage, message::SignedSSVMessage};
use task_executor::{ShutdownReason, TaskExecutor};
use tokio::sync::watch;
use tracing::{error, info};
use types::{Epoch, EthSpec};

#[cfg(test)]
use super::state::DoppelgangerMode;
use super::state::DoppelgangerState;

pub struct OperatorDoppelgangerService<E: EthSpec, S: SlotClock> {
    /// Our operator ID to watch for (wraps database watch)
    own_operator_id: OwnOperatorId,
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
    /// Shutdown sender (triggers fatal shutdown on twin detection)
    shutdown_sender: Mutex<mpsc::Sender<ShutdownReason>>,
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
        own_operator_id: OwnOperatorId,
        slot_clock: S,
        current_epoch: types::Epoch,
        wait_epochs: u64,
        slot_duration: Duration,
        shutdown_sender: mpsc::Sender<ShutdownReason>,
    ) -> (Self, watch::Receiver<bool>) {
        let state = Arc::new(Mutex::new(DoppelgangerState::new()));

        // Create watch channel, starting in monitoring mode
        let (is_monitoring_tx, is_monitoring_rx) = watch::channel(true);

        let monitor_end_epoch = current_epoch + wait_epochs;

        let service = Self {
            own_operator_id,
            state,
            slot_clock,
            monitor_end_epoch,
            slot_duration,
            is_monitoring_tx,
            shutdown_sender: Mutex::new(shutdown_sender),
            _phantom: PhantomData,
        };

        (service, is_monitoring_rx)
    }

    /// Spawn a background task to monitor epoch progression and transition to active mode
    ///
    /// The task first sleeps for the grace period to allow old gossip messages to expire,
    /// then checks the current epoch every slot and automatically calls `transition_to_active()`
    /// when the monitoring period ends.
    ///
    /// # Arguments
    ///
    /// * `grace_period` - Duration to wait before starting twin detection. Should be slightly
    ///   longer than the gossip message cache window (history_length × heartbeat_interval ≈ 4.2s)
    ///   to ensure our own old messages have expired from the network. See `DoppelgangerState`
    ///   documentation for details on why this prevents false positives.
    pub fn spawn_monitor_task(self: Arc<Self>, grace_period: Duration, executor: &TaskExecutor)
    where
        S: 'static,
    {
        executor.spawn_without_exit(
            async move {
                // Wait for grace period first - let old messages expire from gossip cache
                // This prevents false positives from receiving our own old messages after restart
                tokio::time::sleep(grace_period).await;

                // Mark grace period as complete - now we can start detecting twins
                self.state.lock().end_grace_period();

                // Now do normal epoch monitoring
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
            if let Some(operator_id) = self.own_operator_id.get() {
                info!(
                    operator_id = *operator_id,
                    "Operator doppelgänger: monitoring period ended, transitioning to active mode"
                );
            } else {
                info!(
                    "Operator doppelgänger: monitoring period ended, transitioning to active mode"
                );
            }
            // Broadcast the transition - all receivers will see false (not monitoring)
            if let Err(e) = self.is_monitoring_tx.send(false) {
                error!(
                    error = ?e,
                    "Failed to broadcast monitoring transition"
                );
            }
        }
    }

    /// Check if a message indicates a potential doppelgänger (detection logic only)
    ///
    /// Returns `true` if a twin operator is detected, `false` otherwise.
    /// This method performs pure detection logic without side effects (except logging).
    ///
    /// Checks all single-signer messages (QBFT consensus and partial signatures) signed
    /// with our operator ID.
    pub fn is_doppelganger(
        &self,
        signed_message: &SignedSSVMessage,
        qbft_message: Option<&QbftMessage>,
    ) -> bool {
        let state = self.state.lock();

        // Only check in monitor mode (background task handles transition)
        if !state.is_monitoring() {
            return false;
        }

        // Skip check if still in grace period (let old gossip messages expire first)
        // This prevents false positives from receiving our own messages after a restart
        if state.is_in_grace_period() {
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

        // Single-signer message with our operator ID = twin detected!
        let msg_id = signed_message.ssv_message().msg_id();
        error!(
            operator_id = *own_operator_id,
            duty_executor = ?msg_id.duty_executor(),
            height = ?qbft_message.map(|m| m.height),
            round = ?qbft_message.map(|m| m.round),
            qbft_type = ?qbft_message.map(|m| m.qbft_message_type),
            "OPERATOR DOPPELGÄNGER DETECTED: Received message signed with our operator ID. \
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

    /// Get the current mode
    #[cfg(test)]
    #[must_use]
    pub fn mode(&self) -> DoppelgangerMode {
        self.state.lock().mode()
    }

    /// Check if we're still in monitor mode
    ///
    /// Returns `true` if the service is currently in monitoring mode,
    /// `false` if it has transitioned to active mode.
    #[must_use]
    pub fn is_monitoring(&self) -> bool {
        self.state.lock().is_monitoring()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use database::OwnOperatorId;
    use slot_clock::TestingSlotClock;
    use ssv_types::{
        CommitteeId, OperatorId, RSA_SIGNATURE_SIZE,
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
    ) -> OperatorDoppelgangerService<E, TestingSlotClock> {
        let own_operator_id = OwnOperatorId::from(OperatorId(1));
        let genesis_slot = Slot::new(0);
        let genesis_duration = Duration::from_secs(0);
        let slot_duration = Duration::from_secs(12);

        let slot_clock = TestingSlotClock::new(genesis_slot, genesis_duration, slot_duration);

        // Set the clock to the start of current_epoch
        slot_clock.set_slot(current_epoch.start_slot(E::slots_per_epoch()).as_u64());

        // Create a shutdown channel for testing
        let (shutdown_tx, _shutdown_rx) = mpsc::channel(1);

        let (service, _receiver) = OperatorDoppelgangerService::new(
            own_operator_id,
            slot_clock,
            current_epoch,
            wait_epochs,
            slot_duration,
            shutdown_tx,
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
        let service = create_service(Epoch::new(100), 2);
        assert!(service.is_monitoring());
        assert_eq!(service.mode(), DoppelgangerMode::Monitor);
        assert!(service.state.lock().is_in_grace_period());
    }

    // High-value tests for check_message functionality

    #[test]
    fn test_twin_detected_single_signer_with_our_operator_id() {
        let service = create_service(Epoch::new(100), 2);
        let committee_id = CommitteeId([1u8; 32]);

        // End grace period so we can detect twins
        service.state.lock().end_grace_period();

        // Create a single-signer message with our operator ID (1)
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 10, 0);

        // This should detect a twin
        let result = service.is_doppelganger(&signed_message, Some(&qbft_message));
        assert!(
            result,
            "Single-signer message with our operator ID should detect twin"
        );
    }

    #[test]
    fn test_no_twin_during_grace_period() {
        let service = create_service(Epoch::new(100), 2);
        let committee_id = CommitteeId([1u8; 32]);

        // Still in grace period (don't end it)
        assert!(service.state.lock().is_in_grace_period());

        // Create a single-signer message with our operator ID (1)
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 10, 0);

        // This should NOT detect a twin (still in grace period)
        let result = service.is_doppelganger(&signed_message, Some(&qbft_message));
        assert!(
            !result,
            "Message during grace period should NOT detect twin (prevents false positives from own old messages)"
        );
    }

    #[test]
    fn test_no_twin_multi_signer_aggregate_message() {
        let service = create_service(Epoch::new(100), 2);
        let committee_id = CommitteeId([1u8; 32]);

        // End grace period
        service.state.lock().end_grace_period();

        // Create a multi-signer aggregate message (includes our operator ID)
        let (signed_message, qbft_message) = create_test_message(
            committee_id,
            vec![OperatorId(1), OperatorId(2), OperatorId(3)],
            10,
            0,
        );

        // This should NOT detect a twin (aggregate message)
        let result = service.is_doppelganger(&signed_message, Some(&qbft_message));
        assert!(
            !result,
            "Multi-signer aggregate message should NOT detect twin"
        );
    }

    #[test]
    fn test_no_twin_different_operator_id() {
        let service = create_service(Epoch::new(100), 2);
        let committee_id = CommitteeId([1u8; 32]);

        // End grace period
        service.state.lock().end_grace_period();

        // Create a single-signer message from a different operator (2, not 1)
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(2)], 10, 0);

        // This should NOT detect a twin (different operator)
        let result = service.is_doppelganger(&signed_message, Some(&qbft_message));
        assert!(
            !result,
            "Message from different operator should NOT detect twin"
        );
    }

    #[test]
    fn test_no_twin_after_monitoring_period_ends() {
        let service = create_service(Epoch::new(100), 2);
        let committee_id = CommitteeId([1u8; 32]);

        // End grace period
        service.state.lock().end_grace_period();

        // Advance clock beyond monitoring period (100 + 2 = 102)
        service
            .slot_clock
            .set_slot(Epoch::new(102).start_slot(E::slots_per_epoch()).as_u64());

        // Explicitly transition to active (simulating what background task does)
        service.transition_to_active();

        // Create a single-signer message with our operator ID
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 10, 0);

        // This should NOT detect a twin (monitoring period ended)
        let result = service.is_doppelganger(&signed_message, Some(&qbft_message));
        assert!(
            !result,
            "Message after monitoring period should NOT detect twin"
        );
    }
}

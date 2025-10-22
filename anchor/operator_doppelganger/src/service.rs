use std::{sync::Arc, time::Duration};

use database::OwnOperatorId;
use futures::channel::mpsc;
use parking_lot::Mutex;
use ssv_types::{consensus::QbftMessage, message::SignedSSVMessage};
use task_executor::{ShutdownReason, TaskExecutor};
use tracing::{error, info};

use super::state::DoppelgangerState;

pub struct OperatorDoppelgangerService {
    /// Our operator ID to watch for (wraps database watch)
    own_operator_id: OwnOperatorId,
    /// Current state
    state: Arc<Mutex<DoppelgangerState>>,
    /// Number of slots per epoch (for calculating monitoring duration)
    slots_per_epoch: u64,
    /// Duration of a slot (for calculating monitoring duration)
    slot_duration: Duration,
    /// Shutdown sender (triggers fatal shutdown on twin detection)
    shutdown_sender: Mutex<mpsc::Sender<ShutdownReason>>,
}

impl OperatorDoppelgangerService {
    /// Create a new operator doppelgänger service
    pub fn new(
        own_operator_id: OwnOperatorId,
        slots_per_epoch: u64,
        slot_duration: Duration,
        shutdown_sender: mpsc::Sender<ShutdownReason>,
    ) -> Self {
        let state = Arc::new(Mutex::new(DoppelgangerState::new()));

        Self {
            own_operator_id,
            state,
            slots_per_epoch,
            slot_duration,
            shutdown_sender: Mutex::new(shutdown_sender),
        }
    }

    /// Spawn a background task to end monitoring after the configured wait period
    ///
    /// The task sleeps for the grace period plus the monitoring duration, then transitions
    /// to active mode.
    ///
    /// # Arguments
    ///
    /// * `grace_period` - Duration to wait before starting twin detection. Should be slightly
    ///   longer than the gossip message cache window (history_length × heartbeat_interval ≈ 4.2s)
    ///   to ensure our own old messages have expired from the network. See `DoppelgangerState`
    ///   documentation for details on why this prevents false positives.
    /// * `wait_epochs` - Number of epochs to monitor for doppelgängers after grace period ends
    pub fn spawn_monitor_task(
        self: Arc<Self>,
        grace_period: Duration,
        wait_epochs: u64,
        executor: &TaskExecutor,
    ) {
        // Calculate monitoring duration (after grace period)
        let monitoring_duration =
            Duration::from_secs(wait_epochs * self.slots_per_epoch * self.slot_duration.as_secs());

        executor.spawn_without_exit(
            async move {
                // Wait for grace period - prevents false positives from own old messages
                tokio::time::sleep(grace_period).await;

                // Grace period complete - start detecting twins
                self.state.lock().end_grace_period();

                // Wait for monitoring period to complete
                tokio::time::sleep(monitoring_duration).await;

                // Monitoring complete - stop checking for doppelgängers
                self.end_monitoring_period();
            },
            "doppelganger-monitor",
        );
    }

    /// End the monitoring period
    ///
    /// This should be called when the monitoring period completes.
    /// After this, messages will no longer be checked for doppelgängers.
    ///
    /// Note: This is automatically called by `spawn_monitor_task()`. Made public for testing.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn end_monitoring_period(&self) {
        let mut state = self.state.lock();
        if state.is_monitoring() {
            state.end_monitoring();
            info!("Operator doppelgänger: monitoring period ended");
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

        // Skip check if still in grace period
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

    /// Check if we're still in monitor mode
    ///
    /// Returns `true` if the service is currently in monitoring mode,
    /// `false` if it has transitioned to active mode.
    pub fn is_monitoring(&self) -> bool {
        self.state.lock().is_monitoring()
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

    fn create_service() -> OperatorDoppelgangerService {
        let own_operator_id = OwnOperatorId::from(OperatorId(1));
        let slots_per_epoch = 1; // Arbitrary - tests don't use spawn_monitor_task
        let slot_duration = Duration::from_secs(12);

        // Create a shutdown channel for testing
        let (shutdown_tx, _shutdown_rx) = mpsc::channel(1);

        OperatorDoppelgangerService::new(
            own_operator_id,
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
        let service = create_service();
        assert!(service.is_monitoring());
        assert!(service.state.lock().is_in_grace_period());
    }

    // High-value tests for check_message functionality

    #[tokio::test(start_paused = true)]
    async fn test_twin_detected_after_grace_period_timer() {
        let service = Arc::new(create_service());
        let committee_id = CommitteeId([1u8; 32]);
        let executor = create_test_executor();

        // Spawn the monitor task with grace period
        let grace_period = Duration::from_secs(5);
        let wait_epochs = 2;
        service.clone().spawn_monitor_task(grace_period, wait_epochs, &executor);

        // Give the spawned task a chance to start
        tokio::task::yield_now().await;

        // Advance time past grace period
        tokio::time::advance(grace_period).await;

        // Allow multiple yields for the timer to fire and task to process
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }

        // Grace period should be complete
        assert!(!service.state.lock().is_in_grace_period());
        assert!(service.is_monitoring());

        // Create a single-signer message with our operator ID (1)
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 10, 0);

        // This should detect a twin (grace period ended via timer)
        let result = service.is_doppelganger(&signed_message, Some(&qbft_message));
        assert!(
            result,
            "Single-signer message with our operator ID should detect twin after grace period"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_no_twin_during_grace_period() {
        let service = Arc::new(create_service());
        let committee_id = CommitteeId([1u8; 32]);
        let executor = create_test_executor();

        // Spawn the monitor task with grace period
        let grace_period = Duration::from_secs(5);
        let wait_epochs = 2;
        service.clone().spawn_monitor_task(grace_period, wait_epochs, &executor);

        // Still in grace period (don't advance time)
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
        let service = create_service();
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
        let service = create_service();
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

    #[tokio::test(start_paused = true)]
    async fn test_no_twin_after_monitoring_period_timer() {
        let service = Arc::new(create_service());
        let committee_id = CommitteeId([1u8; 32]);
        let executor = create_test_executor();

        // Spawn the monitor task
        let grace_period = Duration::from_secs(5);
        let wait_epochs = 2;
        service.clone().spawn_monitor_task(grace_period, wait_epochs, &executor);

        // Give the spawned task a chance to start
        tokio::task::yield_now().await;

        // Calculate monitoring duration
        let monitoring_duration = Duration::from_secs(wait_epochs * 1 * 12); // epochs * slots_per_epoch * slot_duration

        // Advance time past grace period first
        tokio::time::advance(grace_period).await;

        // Allow the first timer to fire and task to process
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }

        // Now advance time past monitoring period
        tokio::time::advance(monitoring_duration).await;

        // Allow the second timer to fire and task to process
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }

        // Monitoring should be complete
        assert!(!service.is_monitoring());

        // Create a single-signer message with our operator ID
        let (signed_message, qbft_message) =
            create_test_message(committee_id, vec![OperatorId(1)], 10, 0);

        // This should NOT detect a twin (monitoring period ended via timer)
        let result = service.is_doppelganger(&signed_message, Some(&qbft_message));
        assert!(
            !result,
            "Message after monitoring period should NOT detect twin"
        );
    }
}

use std::{marker::PhantomData, sync::Arc};

use parking_lot::Mutex;
use slot_clock::SlotClock;
use ssv_types::{
    OperatorId, consensus::QbftMessage, message::SignedSSVMessage, msgid::DutyExecutor,
};
use tracing::{debug, error, info, warn};
use types::EthSpec;

use super::state::{DoppelgangerMode, DoppelgangerState};

pub struct OperatorDoppelgangerService<E: EthSpec, S: SlotClock> {
    /// Our operator ID to watch for
    own_operator_id: OperatorId,
    /// Current state
    state: Arc<Mutex<DoppelgangerState>>,
    /// Slot clock for epoch tracking
    slot_clock: S,
    /// Enabled flag
    enabled: bool,
    /// Phantom data for EthSpec
    _phantom: PhantomData<E>,
}

impl<E: EthSpec, S: SlotClock> OperatorDoppelgangerService<E, S> {
    /// Create a new operator doppelgänger service
    pub fn new(
        own_operator_id: OperatorId,
        slot_clock: S,
        current_epoch: types::Epoch,
        wait_epochs: u64,
        fresh_k: u64,
        enabled: bool,
    ) -> Self {
        let state = Arc::new(Mutex::new(DoppelgangerState::new(
            current_epoch,
            wait_epochs,
            fresh_k,
        )));

        if enabled {
            info!(
                operator_id = *own_operator_id,
                current_epoch = current_epoch.as_u64(),
                wait_epochs,
                fresh_k,
                "Operator doppelgänger protection enabled, entering monitor mode"
            );
        } else {
            info!("Operator doppelgänger protection disabled");
        }

        Self {
            own_operator_id,
            state,
            slot_clock,
            enabled,
            _phantom: PhantomData,
        }
    }

    /// Check if a message indicates a potential doppelgänger
    ///
    /// Returns true if a twin is detected (should trigger shutdown)
    pub fn check_message(
        &self,
        signed_message: &SignedSSVMessage,
        qbft_message: &QbftMessage,
    ) -> bool {
        if !self.enabled {
            return false;
        }

        // Update mode based on current epoch
        let Some(slot) = self.slot_clock.now() else {
            warn!("Unable to read slot clock, skipping doppelgänger check");
            return false;
        };

        let current_epoch = slot.epoch(E::slots_per_epoch());
        let mut state = self.state.lock();
        state.update_mode(current_epoch);

        // Only check in monitor mode
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

        // Update height and check if the message is fresh
        if !state.update_and_check_freshness(committee_id, qbft_message.height) {
            // Stale message, likely a replay - not evidence of a twin
            debug!(
                operator_id = *self.own_operator_id,
                committee = ?committee_id,
                height = qbft_message.height,
                "Received stale message with our operator ID (likely replay), ignoring"
            );
            return false;
        }

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
    #[allow(dead_code)]
    pub fn mode(&self) -> DoppelgangerMode {
        self.state.lock().mode()
    }

    /// Check if we're still in monitor mode
    #[allow(dead_code)]
    pub fn is_monitoring(&self) -> bool {
        self.enabled && self.state.lock().is_monitoring()
    }
}

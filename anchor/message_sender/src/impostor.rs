use std::sync::Arc;

use slot_clock::SlotClock;
use ssv_types::{CommitteeId, consensus::UnsignedSSVMessage, message::SignedSSVMessage};
use subnet_service::SubnetService;
use tokio::sync::mpsc;
use tracing::debug;

use crate::{Error, MessageCallback, MessageSender};

/// A message sender that logs messages but does not actually send them.
///
/// Used for "impostor" mode debugging/testing. Fork-aware subnet calculation
/// is done via SubnetService to ensure correct routing during fork transitions.
pub struct ImpostorMessageSender<S: SlotClock> {
    // we only hold this so network does not get sad over the closed channel lol
    _network_tx: mpsc::Sender<(String, Vec<u8>)>,
    subnet_service: Arc<SubnetService<S>>,
}

impl<S: SlotClock> Clone for ImpostorMessageSender<S> {
    fn clone(&self) -> Self {
        Self {
            _network_tx: self._network_tx.clone(),
            subnet_service: self.subnet_service.clone(),
        }
    }
}

impl<S: SlotClock> MessageSender for ImpostorMessageSender<S> {
    fn sign_and_send(
        &self,
        msg: UnsignedSSVMessage,
        committee_id: CommitteeId,
        _additional_message_callback: Option<Box<MessageCallback>>,
    ) -> Result<(), Error> {
        let message_slot = require_message_slot(msg.ssv_message.extract_slot())?;

        let subnet = self
            .subnet_service
            .subnet_for_committee_at_slot(committee_id, message_slot)
            .map_err(|e| Error::SubnetCalculation(e.to_string()))?;
        debug!(?msg, ?subnet, "Would send message");
        Ok(())
    }

    fn send(&self, msg: SignedSSVMessage, committee_id: CommitteeId) -> Result<(), Error> {
        let message_slot = require_message_slot(msg.ssv_message().extract_slot())?;

        let subnet = self
            .subnet_service
            .subnet_for_committee_at_slot(committee_id, message_slot)
            .map_err(|e| Error::SubnetCalculation(e.to_string()))?;
        debug!(?msg, ?subnet, "Would send message");
        Ok(())
    }
}

impl<S: SlotClock> ImpostorMessageSender<S> {
    pub fn new(
        network_tx: mpsc::Sender<(String, Vec<u8>)>,
        subnet_service: Arc<SubnetService<S>>,
    ) -> Self {
        Self {
            _network_tx: network_tx,
            subnet_service,
        }
    }
}

fn require_message_slot<T>(slot: Option<T>) -> Result<T, Error> {
    slot.ok_or_else(|| Error::SubnetCalculation("message slot unavailable".to_string()))
}

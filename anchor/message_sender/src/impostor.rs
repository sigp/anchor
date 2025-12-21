use ssv_types::{CommitteeId, consensus::UnsignedSSVMessage, message::SignedSSVMessage};
use subnet_service::SubnetId;
use tokio::sync::mpsc;
use tracing::debug;

use crate::{Error, MessageCallback, MessageSender};

#[derive(Clone)]
pub struct ImpostorMessageSender {
    // we only hold this so network does not get sad over the closed channel lol
    _network_tx: mpsc::Sender<(SubnetId, Vec<u8>)>,
    subnet_count: usize,
}

impl MessageSender for ImpostorMessageSender {
    fn sign_and_send(
        &self,
        msg: UnsignedSSVMessage,
        committee_id: CommitteeId,
        _additional_message_callback: Option<Box<MessageCallback>>,
    ) -> Result<(), Error> {
        let subnet = SubnetId::from_committee_alan(committee_id, self.subnet_count);
        debug!(?msg, ?subnet, "Would send message");
        Ok(())
    }

    fn send(&self, msg: SignedSSVMessage, committee_id: CommitteeId) -> Result<(), Error> {
        let subnet = SubnetId::from_committee_alan(committee_id, self.subnet_count);
        debug!(?msg, ?subnet, "Would send message");
        Ok(())
    }
}

impl ImpostorMessageSender {
    pub fn new(network_tx: mpsc::Sender<(SubnetId, Vec<u8>)>, subnet_count: usize) -> Self {
        Self {
            _network_tx: network_tx,
            subnet_count,
        }
    }
}

#![no_main]

mod setup;

use arbitrary::{Arbitrary, Result, Unstructured};
use gossipsub::{Message, MessageId, TopicHash};
use libfuzzer_sys::fuzz_target;
use libp2p::{
    PeerId,
    identity::{PublicKey, ed25519},
};
use message_receiver::MessageReceiver;
use setup::{RECEIVER, RUNTIME};

#[derive(Debug)]
pub struct ArbitraryPeerId(pub PeerId);
impl<'a> Arbitrary<'a> for ArbitraryPeerId {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        let mut key_bytes = [0u8; 32];
        u.fill_buffer(&mut key_bytes)?;
        let keypair = ed25519::Keypair::try_from_bytes(&mut key_bytes.clone())
            .map_err(|_| arbitrary::Error::IncorrectFormat)?;
        let public_key = PublicKey::from(keypair.public());
        Ok(ArbitraryPeerId(PeerId::from_public_key(&public_key)))
    }
}

#[derive(Debug)]
pub struct ArbitraryMessageId(pub MessageId);
impl<'a> Arbitrary<'a> for ArbitraryMessageId {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        let mut bytes = [0u8; 20];
        u.fill_buffer(&mut bytes)?;
        Ok(ArbitraryMessageId(MessageId::from(&bytes[..])))
    }
}

#[derive(Debug)]
pub struct ArbitraryMessage(pub Message);
impl<'a> Arbitrary<'a> for ArbitraryMessage {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        let data_len = u.int_in_range(1..=1024)?;
        let mut data = vec![0u8; data_len];
        u.fill_buffer(&mut data)?;

        let topic_seed: [u8; 8] = u.arbitrary()?;
        let topic_str = format!("ssv.v2.{}", hex::encode(topic_seed));
        let topic = TopicHash::from_raw(topic_str);

        Ok(ArbitraryMessage(Message {
            source: None,
            data,
            sequence_number: None,
            topic,
        }))
    }
}

// Composite fuzz input
#[derive(Arbitrary, Debug)]
struct FuzzInput {
    peer_id: ArbitraryPeerId,
    message_id: ArbitraryMessageId,
    message: ArbitraryMessage,
}

fuzz_target!(|input: FuzzInput| {
    let (peer_id, message_id, message) = (input.peer_id.0, input.message_id.0, input.message.0);
    RUNTIME.block_on(async {
        let _ = RECEIVER.receive(peer_id, message_id, message);
    });
});

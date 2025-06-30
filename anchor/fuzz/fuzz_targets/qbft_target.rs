#![no_main]

mod setup;
use arbitrary::{Arbitrary, Result, Unstructured};
use libfuzzer_sys::fuzz_target;
use qbft::WrappedQbftMessage;
use setup::QBFT;
use sha2::{Digest, Sha256};
use ssv_types::{
    IndexSet, OperatorId,
    consensus::{BeaconVote, QbftMessage, QbftMessageType},
    message::{MsgType, RSA_SIGNATURE_SIZE, SSVMessage, SignedSSVMessage},
    msgid::MessageId,
};
use ssz::Encode;
use types::Hash256;

#[derive(Debug)]
pub struct ArbitraryWrappedQbftMessage(pub WrappedQbftMessage);
impl<'a> Arbitrary<'a> for ArbitraryWrappedQbftMessage {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        // Generate message type
        let qbft_message_type = match u.int_in_range(0..=3)? {
            0 => QbftMessageType::Proposal,
            1 => QbftMessageType::Prepare,
            2 => QbftMessageType::Commit,
            _ => QbftMessageType::RoundChange,
        };

        let round = u.int_in_range(1..=12)?;
        let height = u.int_in_range(1..=10)?;
        let msg_id = MessageId::from([0u8; 56]);

        // Create committee with valid operators
        let mut committee = IndexSet::new();
        for i in 1..=4 {
            committee.insert(OperatorId(i));
        }

        // Determine the leader based on round robin algorithm
        let first_round_index = height % committee.len() as u64;
        let leader_index = (first_round_index + round - 1) % committee.len() as u64;
        let leader = committee
            .get_index(leader_index as usize)
            .copied()
            .unwrap_or(OperatorId(1));

        // Create a valid BeaconVote
        let beacon_vote = BeaconVote {
            block_root: Hash256::from_slice(&u.bytes(32)?[..32]),
            source: types::Checkpoint::default(),
            target: types::Checkpoint::default(),
        };

        // Encode the beacon vote
        let full_data = if qbft_message_type == QbftMessageType::Proposal {
            // Only proposals should have full data
            beacon_vote.as_ssz_bytes()
        } else {
            Vec::new() // Other message types have empty full_data
        };

        let mut operator_ids = Vec::new();
        if qbft_message_type == QbftMessageType::Proposal {
            operator_ids.push(leader);
        } else if qbft_message_type == QbftMessageType::Commit && u.arbitrary()? {
            // Decide how many signers (must meet quorum)
            let f = (committee.len() - 1) / 3;
            let quorum_size = 2 * f + 1;

            // Add enough operators to meet quorum
            let mut op_iter = committee.iter();
            for _ in 0..quorum_size {
                if let Some(&op) = op_iter.next() {
                    operator_ids.push(op);
                }
            }

            // Sort to satisfy validation
            operator_ids.sort();
        } else {
            let op_idx = u.int_in_range(0..=committee.len() - 1)?;
            operator_ids.push(*committee.get_index(op_idx).unwrap());
        }

        // Generate matching number of signatures
        let signatures = operator_ids
            .iter()
            .map(|_| vec![0u8; RSA_SIGNATURE_SIZE])
            .collect::<Vec<_>>();

        let root = if !full_data.is_empty() {
            let mut hasher = Sha256::new();
            hasher.update(&full_data);
            Hash256::from(hasher.finalize().as_ref())
        } else {
            // For messages without data, use a valid default
            Hash256::from_slice(&u.bytes(32)?[..32])
        };

        let prepare_justification = Vec::new();
        let round_change_justification = Vec::new();

        // Create QbftMessage
        let qbft_message = QbftMessage {
            qbft_message_type,
            height,
            round,
            identifier: (&msg_id).into(),
            root,
            data_round: u.int_in_range(0..=round)?,
            round_change_justification,
            prepare_justification,
        };

        // Create SSV Message
        let ssv_message = SSVMessage::new(
            MsgType::SSVConsensusMsgType,
            msg_id,
            qbft_message.as_ssz_bytes(),
        )
        .expect("Failed to create SSVMessage");

        let signed_message =
            SignedSSVMessage::new(signatures, operator_ids, ssv_message, full_data)
                .expect("Failed to create SignedSSVMessage");

        // Return the final WrappedQbftMessage
        Ok(ArbitraryWrappedQbftMessage(WrappedQbftMessage {
            signed_message,
            qbft_message,
        }))
    }
}

// Fuzz message validation
fuzz_target!(|msg: ArbitraryWrappedQbftMessage| {
    let mut qbft_locked = QBFT.lock().unwrap();
    qbft_locked.receive(msg.0)
});

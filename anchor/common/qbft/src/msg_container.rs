use std::collections::{HashMap, HashSet};

use ssv_types::OperatorId;
use types::Hash256;

use crate::{Round, WrappedQbftMessage};

/// Message container with strong typing and validation
#[derive(Default)]
pub struct MessageContainer {
    /// Messages stored as a Vec per round to preserve insertion order
    messages: HashMap<Round, Vec<WrappedQbftMessage>>,
    /// Track which operators have sent messages for each round
    senders_by_round: HashMap<Round, HashSet<OperatorId>>,
    /// Track unique values per round
    values_by_round: HashMap<Round, HashSet<Hash256>>,
    /// The quorum size for the qbft instance
    quorum_size: usize,
}

impl MessageContainer {
    /// Construct a new MessageContainer with a specific quorum size
    pub fn new(quorum_size: usize) -> Self {
        Self {
            quorum_size,
            messages: HashMap::new(),
            senders_by_round: HashMap::new(),
            values_by_round: HashMap::new(),
        }
    }

    /// Add a new message to the container for the round
    pub fn add_message(
        &mut self,
        round: Round,
        sender: OperatorId,
        msg: &WrappedQbftMessage,
    ) -> bool {
        // Check if we already have a message from this sender for this round
        let senders = self.senders_by_round.entry(round).or_default();
        if !senders.insert(sender) {
            return false;
        }

        self.messages.entry(round).or_default().push(msg.clone());

        self.values_by_round
            .entry(round)
            .or_default()
            .insert(msg.qbft_message.root);

        true
    }

    /// Check if we have a quorum of messages for the round. If so, return the hash of the value
    /// with the quorum
    pub fn has_quorum(&self, round: Round) -> Option<Hash256> {
        let round_messages = self.messages.get(&round)?;

        // Count occurrences of each value
        let mut value_counts: HashMap<Hash256, usize> = HashMap::new();
        for msg in round_messages {
            *value_counts.entry(msg.qbft_message.root).or_default() += 1;
        }

        // Find any value that has reached quorum
        value_counts
            .into_iter()
            .find(|(_, count)| *count >= self.quorum_size)
            .map(|(value, _)| value)
    }

    /// Count the number of messages we have received for this round
    pub fn num_messages_for_round(&self, round: Round) -> usize {
        self.messages
            .get(&round)
            .map(|msgs| msgs.len())
            .unwrap_or(0)
    }

    /// If we have a quorum for the round, get all of the messages that correspond to that quorum
    pub fn get_quorum_of_messages(&self, round: Round) -> Vec<WrappedQbftMessage> {
        let mut msgs = vec![];
        // collect all of the messages where root = quorum hash
        if let Some(hash) = self.has_quorum(round)
            && let Some(round_messages) = self.messages.get(&round)
        {
            for msg in round_messages {
                if msg.qbft_message.root == hash {
                    msgs.push(msg.clone());
                }
            }
        }
        msgs
    }

    /// Gets all messages for a specific round
    pub fn get_messages_for_round(&self, round: Round) -> Vec<&WrappedQbftMessage> {
        self.messages
            .get(&round)
            .map(|round_messages| round_messages.iter().collect())
            .unwrap_or_default()
    }
}

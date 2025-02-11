use crate::{Round, WrappedQbftMessage};
use ssv_types::OperatorId;
use std::collections::{HashMap, HashSet};
use types::Hash256;

/// Message container with strong typing and validation
#[derive(Default)]
pub struct MessageContainer {
    /// Messages indexed by round and then by sender
    messages: HashMap<Round, HashMap<OperatorId, WrappedQbftMessage>>,
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
        if self
            .messages
            .get(&round)
            .and_then(|msgs| msgs.get(&sender))
            .is_some()
        {
            return false; // Duplicate message
        }

        // Add message and track its value
        self.messages
            .entry(round)
            .or_default()
            .insert(sender, msg.clone());

        self.values_by_round
            .entry(round)
            .or_default()
            .insert(msg.qbft_message.root);

        true
    }

    /// Check if we have a quorum of messages for the round. If so, return the hash of the value with
    /// the quorum
    pub fn has_quorum(&self, round: Round) -> Option<Hash256> {
        let round_messages = self.messages.get(&round)?;

        // Count occurrences of each value
        let mut value_counts: HashMap<Hash256, usize> = HashMap::new();
        for msg in round_messages.values() {
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

    /// Gets all messages for a specific round
    pub fn get_messages_for_round(&self, round: Round) -> Vec<&WrappedQbftMessage> {
        // If we have messages for this round in our container, return them all
        // If not, return an empty vector
        self.messages
            .get(&round)
            .map(|round_messages| {
                // Convert the values of the HashMap into a Vec
                round_messages.values().collect()
            })
            .unwrap_or_default()
    }
}

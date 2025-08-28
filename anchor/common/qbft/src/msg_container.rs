use std::collections::{BTreeMap, HashMap, HashSet};

use ssv_types::OperatorId;
use types::Hash256;

use crate::{Round, WrappedQbftMessage};

/// Message container with strong typing and validation
#[derive(Default)]
pub struct MessageContainer {
    /// Messages stored as a Vec per round to preserve insertion order
    messages: HashMap<Round, Vec<WrappedQbftMessage>>,
    /// Track which operators have sent messages for each round
    senders_by_round: BTreeMap<Round, HashSet<OperatorId>>,
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
            senders_by_round: BTreeMap::new(),
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

        let mut msg = msg.clone();
        // We no longer have need for full data in these messages, as the hash is stored the QBFT
        // state. The messages are here only used for quorum checking, and for justifications. For
        // both the data is not needed.
        msg.signed_message.set_full_data(vec![]);

        self.values_by_round
            .entry(round)
            .or_default()
            .insert(msg.qbft_message.root);

        // Add message and track its value
        self.messages.entry(round).or_default().push(msg);

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
    pub fn highest_partial_quorum_above_round(
        &self,
        round: Round,
        partial: usize,
    ) -> Option<Round> {
        // Collect all operators from rounds > round
        let mut all_operators = HashSet::new();
        let mut min_future_round = None;

        for (&r, operators) in self.senders_by_round.range((round + 1)..) {
            // Track minimum round
            if min_future_round.is_none() {
                min_future_round = Some(r);
            }

            // Add all operators from this round
            for &operator in operators {
                all_operators.insert(operator);
            }
        }

        // If we have partial quorum, return the minimum round
        if all_operators.len() >= partial {
            min_future_round
        } else {
            None
        }
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

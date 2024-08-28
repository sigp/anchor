use futures::prelude::Stream;
use smallvec::SmallVec;
use std::collections::HashMap;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use tracing::warn;

type ValidationId = usize;
type Round = usize;
/// The structure that defines the QBFT thing
pub struct QBFT {
    id: usize,
    quorum_size: usize,
    /// This the round of the current QBFT
    round: usize,
    leader: usize,
    current_validation_id: usize,
    inflight_validations: HashMap<ValidationId, ValidationMessage>, // TODO: Potentially unbounded
    /// Contains the events to be sent out
    out_events: SmallVec<[OutputMessage; 5]>,

    /// The current messages for this round
    prepare_messages: HashMap<Round, Vec<PrepareMessage>>,
    /// This stores the task context, so we can wake up the task when we are adding events to the
    /// queue.
    waker: Option<Waker>,
}

pub enum OutputMessage {
    Propose(ProposeMessage),
    Prepare(PrepareMessage),
    Confirm(ConfirmMessage),
    Validate(ValidationMessage),
}

pub struct ProposeMessage {
    value: Vec<u8>,
}

#[derive(Clone)]
pub struct PrepareMessage {
    value: Vec<u8>,
}

pub struct ConfirmMessage {
    value: Vec<u8>,
}

pub struct ValidationMessage {
    id: ValidationId,
    value: Vec<u8>,
    round: usize,
}

pub enum ValidationOutcome {
    Success,
    Failure(ValidationError),
}

/// This is the kind of error that can happen on a validation
pub enum ValidationError {
    /// This means that lighthouse couldn't find the value
    LighthouseSaysNo,
    /// It doesn't exist and its wrong
    DidNotExist,
}

impl QBFT {
    pub fn new(round: usize, leader: usize, quorum_size: usize) -> Self {
        // Validate Quorum size, cannot be 0
        QBFT {
            id: 0,
            quorum_size,
            round,
            leader,
            current_validation_id: 0,
            inflight_validations: HashMap::with_capacity(100),
            prepare_messages: HashMap::with_capacity(quorum_size),
            out_events: SmallVec::new(),
            waker: None,
        }
    }

    pub fn initialize(&mut self) {
        if self.is_leader(self.id) {
            self.add_event(OutputMessage::Propose(ProposeMessage { value: vec![1] }));
        }
    }

    pub fn increment_round(&mut self) {}

    /// We have received a proposal from someone. We need to:
    ///
    /// 1. Check the proposer is a valid and who we expect
    /// 2. Check that the proposal is valid and we agree on the value
    pub fn received_propose(&mut self, value: Vec<u8>, leader: usize) {
        // Handle step 1.
        if !self.is_leader(leader) {
            return;
        }

        // Step 2
        self.add_event(OutputMessage::Validate(ValidationMessage {
            id: self.current_validation_id,
            value,
            round: self.round,
        }));
        self.current_validation_id += 1;
    }

    /// The response to a validation request.
    ///
    /// If the proposal fails we drop the message, if it is successful, we send a prepare.
    pub fn validate_proposal(&mut self, validation_id: ValidationId, outcome: ValidationOutcome) {
        let Some(validation_message) = self.inflight_validations.remove(&validation_id) else {
            warn!(validation_id, "Validation response without a request");
            return;
        };

        let prepare_message = PrepareMessage {
            value: validation_message.value,
        };
        self.add_event(OutputMessage::Prepare(prepare_message.clone()));

        // TODO: Store a prepare locally
        self.prepare_messages
            .entry(self.round)
            .or_default()
            .push(prepare_message);
    }

    /// We have received a PREPARE message
    ///
    /// If we have reached quorum then send a CONFIRM
    /// Otherwise store the prepare and wait for quorum.
    pub fn received_prepare(&mut self, prepare_message: PrepareMessage) {
        // Some kind of validation, legit person? legit group, legit round?

        // Make sure the value matches

        // Store the prepare we received
        self.prepare_messages
            .entry(self.round)
            .or_default()
            .push(prepare_message);

        // Check Quorum
        // This is based on number of nodes in the group.
        // TODO: Prob need to be more robust here
        // We have to make sure the value on all the prepare's match
        if let Some(messages) = self.prepare_messages.get(&self.round) {
            if messages.len() >= self.quorum_size {
                // SEND CONFIRM
            }
        }
    }

    pub fn get_messages(&mut self) {}

    // pub enum ExternalMessage
    //

    // Internal helper functions
    fn add_event(&mut self, event: OutputMessage) {
        self.out_events.push(event);
        if let Some(waker) = &self.waker {
            waker.wake_by_ref();
        }
    }

    fn is_leader(&mut self, questionable_leader: usize) -> bool {
        questionable_leader % self.round == 0
    }
}

impl Stream for QBFT {
    type Item = OutputMessage;

    // NOTE: Wake up the task on events
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(waker) = &self.waker {
            if waker.will_wake(cx.waker()) {
                self.waker = Some(cx.waker().clone());
            }
        } else {
            self.waker = Some(cx.waker().clone());
        }

        if !self.out_events.is_empty() {
            return Poll::Ready(Some(self.out_events.remove(0)));
        } else {
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_initialization() {
        let mut qbft = QBFT::new(0, 0, 4);
        qbft.initialize();

        let _out_message = qbft.next().await;
    }

    #[tokio::test]
    async fn test_happy_case_two_nodes() {
        let mut qbft1 = QBFT::new(0, 0, 4);
        qbft1.initialize();
        let mut qbft2 = QBFT::new(1, 0, 4);
        qbft2.initialize();

        match qbft1.next().await {
            Some(OutputMessage::Propose(propose_message)) => {
                qbft2.received_propose(propose_message.value, 0);
            }
            _ => unreachable!("This is bad code, should match on everything, but its a test"),
        }
    }

    //
}

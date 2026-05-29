//! Instrumentation taxonomy for QBFT Manager.
#![expect(
    dead_code,
    reason = "Expected to be implemented by proposer QBFT instrumentation"
)]

use std::mem::{Discriminant, discriminant};

use qbft::InstanceState;
use ssv_types::Round;

pub mod checkpoints {
    pub const QBFT_INSTANCE_STARTED: &str = "qbft_instance_started";
    pub const QBFT_PROPOSAL_ACCEPTED: &str = "qbft_proposal_accepted";
    pub const QBFT_PREPARE_QUORUM: &str = "qbft_prepare_quorum";
    pub const QBFT_ROUND_ADVANCE: &str = "qbft_round_advance";
    pub const QBFT_DECIDED: &str = "qbft_decided";
    pub const QBFT_TIMED_OUT: &str = "qbft_timed_out";
    pub const QBFT_CHANNEL_CLOSED: &str = "qbft_channel_closed";
}

/// Reason why a QBFT round advanced, determined by the boundary layer
/// through before/after state observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundAdvanceReason {
    /// The local round timer expired without reaching consensus.
    Timeout,
    /// The node observed f+1 round-change messages from peers at a higher
    /// round.
    FPlusOneRoundChange,
    /// A full quorum of round-change messages was observed, achieving
    /// round-change consensus.
    RoundChangeQuorum,
}

impl RoundAdvanceReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::FPlusOneRoundChange => "f_plus_1_rc",
            Self::RoundChangeQuorum => "rc_quorum",
        }
    }
}

/// Identifies which branch of the recv loop observed the round advance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvArmTag {
    /// A network message was received and processed.
    Message,
    /// The local round timer expired and `end_round` was called.
    RoundEnd,
}

/// Classify a round advance observed at the boundary layer.
///
/// This function uses `std::mem::Discriminant<InstanceState>` to avoid comparing the
/// `proposal_root` payload of certain states or modifying internal API components. Only the state
/// variant is relevant here.
///
/// Returns `None` if the round did not actually advance and `Some(reason)` when it did.
pub fn classify_round_advance(
    _before_state: Discriminant<InstanceState>,
    after_state: Discriminant<InstanceState>,
    recv_arm: RecvArmTag,
    before_round: Round,
    after_round: Round,
) -> Option<RoundAdvanceReason> {
    if after_round <= before_round {
        return None;
    }

    let reason = match recv_arm {
        RecvArmTag::RoundEnd => RoundAdvanceReason::Timeout,
        RecvArmTag::Message => {
            if after_state == discriminant(&InstanceState::SentRoundChange) {
                RoundAdvanceReason::FPlusOneRoundChange
            } else {
                RoundAdvanceReason::RoundChangeQuorum
            }
        }
    };

    Some(reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disc(state: InstanceState) -> Discriminant<InstanceState> {
        discriminant(&state)
    }

    #[test]
    fn no_advance_when_round_unchanged() {
        let result = classify_round_advance(
            disc(InstanceState::AwaitingProposal),
            disc(InstanceState::AwaitingProposal),
            RecvArmTag::Message,
            Round::from(1u64),
            Round::from(1u64),
        );
        assert_eq!(
            result, None,
            "should return None when after_round == before_round (no advance occurred)"
        );
    }

    #[test]
    fn no_advance_when_round_decreased() {
        let result = classify_round_advance(
            disc(InstanceState::AwaitingProposal),
            disc(InstanceState::AwaitingProposal),
            RecvArmTag::Message,
            Round::from(3u64),
            Round::from(2u64),
        );
        assert_eq!(
            result, None,
            "should return None when after_round < before_round (defensive case)"
        );
    }

    #[test]
    fn timeout_on_round_end() {
        let result = classify_round_advance(
            disc(InstanceState::AwaitingProposal),
            disc(InstanceState::AwaitingProposal),
            RecvArmTag::RoundEnd,
            Round::from(1u64),
            Round::from(2u64),
        );
        assert_eq!(
            result,
            Some(RoundAdvanceReason::Timeout),
            "RoundEnd arm should always classify as Timeout regardless of state"
        );
    }

    #[test]
    fn f_plus_one_rc_when_after_state_is_sent_round_change() {
        let result = classify_round_advance(
            disc(InstanceState::AwaitingProposal),
            disc(InstanceState::SentRoundChange),
            RecvArmTag::Message,
            Round::from(1u64),
            Round::from(3u64),
        );
        assert_eq!(
            result,
            Some(RoundAdvanceReason::FPlusOneRoundChange),
            "Message arm with after_state == SentRoundChange means f+1 peers pulled us forward"
        );
    }

    #[test]
    fn rc_quorum_when_after_state_is_awaiting_proposal() {
        let result = classify_round_advance(
            disc(InstanceState::SentRoundChange),
            disc(InstanceState::AwaitingProposal),
            RecvArmTag::Message,
            Round::from(2u64),
            Round::from(3u64),
        );
        assert_eq!(
            result,
            Some(RoundAdvanceReason::RoundChangeQuorum),
            "Message arm with after_state != SentRoundChange means full quorum was reached"
        );
    }

    #[test]
    fn rc_quorum_when_after_state_is_round_change_consensus() {
        let result = classify_round_advance(
            disc(InstanceState::SentRoundChange),
            disc(InstanceState::RoundChangeConsensus),
            RecvArmTag::Message,
            Round::from(2u64),
            Round::from(3u64),
        );
        assert_eq!(
            result,
            Some(RoundAdvanceReason::RoundChangeQuorum),
            "RoundChangeConsensus state also indicates full quorum was observed"
        );
    }

    #[test]
    fn timeout_regardless_of_state() {
        for state in [
            InstanceState::AwaitingProposal,
            InstanceState::SentRoundChange,
            InstanceState::Complete,
            InstanceState::RoundChangeConsensus,
        ] {
            let result = classify_round_advance(
                disc(state),
                disc(state),
                RecvArmTag::RoundEnd,
                Round::from(1u64),
                Round::from(2u64),
            );
            assert_eq!(
                result,
                Some(RoundAdvanceReason::Timeout),
                "RoundEnd arm must always produce Timeout, but failed for state {:?}",
                state
            );
        }
    }
}

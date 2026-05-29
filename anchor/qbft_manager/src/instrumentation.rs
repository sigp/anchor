//! Instrumentation taxonomy for QBFT Manager.
#![expect(
    dead_code,
    reason = "Expected to be implemented by proposer QBFT instrumentation"
)]

use qbft::InstanceStateKind;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoundAdvanceReason {
    /// The local round timer expired without reaching consensus.
    Timeout,
    /// The node observed f+1 round-change messages from peers at a higher
    /// round.
    FPlusOneRoundChange,
    /// A full quorum of round-change messages was observed, achieving
    /// round-change consensus.
    RoundChangeQuorum,
    /// A justified proposal for a higher round arrived, pulling the instance
    /// forward to catch up with the leader's round (no round change occurred).
    FutureRoundProposal,
}

impl RoundAdvanceReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::FPlusOneRoundChange => "f_plus_1_rc",
            Self::RoundChangeQuorum => "rc_quorum",
            Self::FutureRoundProposal => "future_proposal",
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
/// Consumes the payload-free `InstanceStateKind` of the after-state so the classifier depends
/// only on the state variant without concern for state attributes.
///
/// `Message` match arm after-state kind captures how the round advanced:
/// - `SentRoundChange`: f+1 peers at a higher round pulled us forward.
/// - `Prepare`: a justified future-round proposal arrived (we caught up to the leader, no round
///   change occurred).
/// - anything else (e.g. `AwaitingProposal`, `RoundChangeConsensus`): a full round-change quorum
///   was reached.
///
/// Returns `None` if the round did not actually advance and `Some(reason)` when it did.
pub fn classify_round_advance(
    _before_state: InstanceStateKind,
    after_state: InstanceStateKind,
    recv_arm: RecvArmTag,
    before_round: Round,
    after_round: Round,
) -> Option<RoundAdvanceReason> {
    if after_round <= before_round {
        return None;
    }

    let reason = match recv_arm {
        RecvArmTag::RoundEnd => RoundAdvanceReason::Timeout,
        RecvArmTag::Message => match after_state {
            InstanceStateKind::SentRoundChange => RoundAdvanceReason::FPlusOneRoundChange,
            InstanceStateKind::Prepare => RoundAdvanceReason::FutureRoundProposal,
            _ => RoundAdvanceReason::RoundChangeQuorum,
        },
    };

    Some(reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_advance_when_round_unchanged() {
        let result = classify_round_advance(
            InstanceStateKind::AwaitingProposal,
            InstanceStateKind::AwaitingProposal,
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
            InstanceStateKind::AwaitingProposal,
            InstanceStateKind::AwaitingProposal,
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
            InstanceStateKind::AwaitingProposal,
            InstanceStateKind::AwaitingProposal,
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
            InstanceStateKind::AwaitingProposal,
            InstanceStateKind::SentRoundChange,
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
            InstanceStateKind::SentRoundChange,
            InstanceStateKind::AwaitingProposal,
            RecvArmTag::Message,
            Round::from(2u64),
            Round::from(3u64),
        );
        assert_eq!(
            result,
            Some(RoundAdvanceReason::RoundChangeQuorum),
            "Message arm landing in AwaitingProposal (neither SentRoundChange nor Prepare) means \
             full quorum was reached"
        );
    }

    #[test]
    fn rc_quorum_when_after_state_is_round_change_consensus() {
        let result = classify_round_advance(
            InstanceStateKind::SentRoundChange,
            InstanceStateKind::RoundChangeConsensus,
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
    fn future_proposal_when_after_state_is_prepare() {
        let result = classify_round_advance(
            InstanceStateKind::AwaitingProposal,
            InstanceStateKind::Prepare,
            RecvArmTag::Message,
            Round::from(1u64),
            Round::from(3u64),
        );
        assert_eq!(
            result,
            Some(RoundAdvanceReason::FutureRoundProposal),
            "Message arm landing in Prepare means a justified future-round proposal pulled the \
             instance forward to catch up with the leader (no round change occurred), not a \
             round-change quorum"
        );
    }

    #[test]
    fn timeout_regardless_of_state() {
        let after_states = [
            InstanceStateKind::AwaitingProposal,
            InstanceStateKind::Prepare,
            InstanceStateKind::SentRoundChange,
            InstanceStateKind::Complete,
            InstanceStateKind::RoundChangeConsensus,
        ];

        for after_state in after_states {
            let result = classify_round_advance(
                InstanceStateKind::AwaitingProposal,
                after_state,
                RecvArmTag::RoundEnd,
                Round::from(1u64),
                Round::from(2u64),
            );
            assert_eq!(
                result,
                Some(RoundAdvanceReason::Timeout),
                "RoundEnd arm must always produce Timeout, but failed for after_state {after_state:?}"
            );
        }
    }
}

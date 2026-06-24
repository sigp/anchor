//! Instrumentation taxonomy for QBFT Manager.

use qbft::InstanceStateKind;
use ssv_types::Round;
use tokio::time::Instant;
use tracing::{Span, field, info, info_span};

use crate::metrics;

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
    /// A justified proposal for a higher round arrived, pulling the instance
    /// forward to catch up with the leader's round (no round change occurred).
    FutureRoundProposal,
    /// A full quorum of round-change messages was observed, achieving
    /// round-change consensus.
    RoundChangeQuorum,
}

impl RoundAdvanceReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::FPlusOneRoundChange => "f_plus_1_rc",
            Self::FutureRoundProposal => "future_proposal",
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
/// Consumes the payload-free `InstanceStateKind` of the after-state so the classifier depends
/// only on the state variant without concern for state attributes. Returns `None` if the round
/// did not actually advance and `Some(reason)` when it did.
///
/// `Message`-arm after-states coincide with a
/// round advance:
/// - `SentRoundChange`: f+1 peers at a higher round pulled us forward.
/// - `Prepare`: a justified future-round proposal arrived.
/// - `AwaitingProposal` or `RoundChangeConsensus`: a full round-change quorum was observed.
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
            // A Message-arm round advance into AwaitingProposal is the round-change quorum path:
            // received_round_change sets RoundChangeConsensus, then set_round() calls
            // start_round(), which leaves both leader and non-leader instances in
            // AwaitingProposal.
            InstanceStateKind::AwaitingProposal | InstanceStateKind::RoundChangeConsensus => {
                RoundAdvanceReason::RoundChangeQuorum
            }
            InstanceStateKind::Commit | InstanceStateKind::Complete => return None,
        },
    };

    Some(reason)
}

/// Terminal outcome of a proposer QBFT instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposerOutcome {
    /// Consensus was reached.
    Decided,
    /// The instance exhausted its rounds without deciding.
    MaxRoundTimeout,
    /// The message channel closed before the instance decided (instance cleaned up).
    ChannelClosed,
}

impl ProposerOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Decided => "decided",
            Self::MaxRoundTimeout => "max_round_timeout",
            Self::ChannelClosed => "channel_closed",
        }
    }

    fn checkpoint(self) -> &'static str {
        match self {
            Self::Decided => checkpoints::QBFT_DECIDED,
            Self::MaxRoundTimeout => checkpoints::QBFT_TIMED_OUT,
            Self::ChannelClosed => checkpoints::QBFT_CHANNEL_CLOSED,
        }
    }
}

/// Boundary-layer observer for a single proposer QBFT instance.
///
/// Owns a proposer duty instrumentation span and a QBFT instance start time. Serves as a single place
/// where proposer lifecycle tracing events and metrics are emitted.
pub struct ProposerObserver {
    span: Span,
    started: Instant,
}

impl ProposerObserver {
    /// Open the instance span, record the handoff budget (if known), and emit the start checkpoint.
    pub fn start(instance_height: u64, handoff_budget_ms: Option<u64>) -> Self {
        let span = info_span!(
            "proposer_qbft_instance",
            role = "proposer",
            instance_height,
            handoff_budget_ms = field::Empty,
            decided_round = field::Empty,
            outcome = field::Empty,
            duration_ms = field::Empty,
        );

        if let Some(budget_ms) = handoff_budget_ms {
            span.record("handoff_budget_ms", budget_ms);
            metrics::observe(
                &metrics::PROPOSER_QBFT_HANDOFF_BUDGET_SECONDS,
                budget_ms as f64 / 1000.0,
            );
        }

        span.in_scope(|| {
            info!(
                checkpoint = checkpoints::QBFT_INSTANCE_STARTED,
                "Proposer QBFT instance started"
            );
        });

        Self {
            span,
            started: Instant::now(),
        }
    }

    /// Classify a round boundary and, if the round advanced, emit the round-advance event and bump
    /// the per-reason counter.
    pub fn observe_round_advance(
        &self,
        before_state: InstanceStateKind,
        after_state: InstanceStateKind,
        recv_arm: RecvArmTag,
        from: Round,
        to: Round,
    ) {
        if let Some(reason) = classify_round_advance(before_state, after_state, recv_arm, from, to)
        {
            self.span.in_scope(|| {
                info!(
                    checkpoint = checkpoints::QBFT_ROUND_ADVANCE,
                    reason = reason.as_str(),
                    from_round = u64::from(from),
                    to_round = u64::from(to),
                    "Proposer QBFT round advance"
                );
            });
            metrics::inc_counter_vec(&metrics::PROPOSER_ROUND_ADVANCE_TOTAL, &[reason.as_str()]);
        }
    }

    /// Emit a stage-transition event when the instance changes state into `Prepare` or `Commit`
    /// within the same round. A no-op when the state is unchanged or transitions into a variant
    /// that is not interesting at this layer.
    pub fn observe_stage_transition(&self, before: InstanceStateKind, after: InstanceStateKind) {
        if before == after {
            return;
        }
        self.span.in_scope(|| match after {
            InstanceStateKind::Prepare => info!(
                checkpoint = checkpoints::QBFT_PROPOSAL_ACCEPTED,
                "Proposal accepted, entering Prepare"
            ),
            InstanceStateKind::Commit => info!(
                checkpoint = checkpoints::QBFT_PREPARE_QUORUM,
                "Prepare quorum reached, entering Commit"
            ),
            _ => {}
        });
    }

    /// Record the terminal outcome on the span, emit the completion checkpoint, and observe the
    /// decided-round, duration, and outcome metrics.
    pub fn finish(&self, outcome: ProposerOutcome, decided_round: u64) {
        let duration = self.started.elapsed();
        let duration_ms = duration.as_millis() as u64;

        self.span.record("decided_round", decided_round);
        self.span.record("outcome", outcome.as_str());
        self.span.record("duration_ms", duration_ms);
        self.span.in_scope(|| {
            info!(
                checkpoint = outcome.checkpoint(),
                decided_round,
                duration_ms,
                outcome = outcome.as_str(),
                "Proposer QBFT instance finished"
            );
        });

        metrics::observe(&metrics::PROPOSER_QBFT_DECIDED_ROUND, decided_round as f64);
        metrics::observe(
            &metrics::PROPOSER_QBFT_DURATION_SECONDS,
            duration.as_secs_f64(),
        );
        metrics::inc_counter_vec(&metrics::PROPOSER_QBFT_OUTCOME_TOTAL, &[outcome.as_str()]);
    }
}

#[cfg(test)]
mod tests {
    use bls::FixedBytesExtended;
    use qbft::{
        ConfigBuilder, DefaultLeaderFunction, InstanceHeight, InstanceStateKind, Qbft,
        WrappedQbftMessage,
    };
    use ssv_types::{
        OperatorId, RSA_SIGNATURE_SIZE, Round, VariableList,
        consensus::{BeaconVote, NoDataValidation, QbftData, QbftMessage, QbftMessageType},
        message::{MsgType, SSVMessage, SignedSSVMessage},
        msgid::MessageId,
    };
    use ssz::Encode;
    use types::{Checkpoint, Hash256};

    use super::{RecvArmTag, RoundAdvanceReason, classify_round_advance};

    /// Constructs a fake signed QBFT network message that can be fed into `instance.receive`.
    fn build_wrapped_msg(
        msg_type: QbftMessageType,
        round: u64,
        root: Hash256,
        data_round: u64,
        signer: u64,
        rc_justifications: Vec<
            VariableList<u8, ssv_types::consensus::RoundChangeJustificationLength>,
        >,
        full_data: Vec<u8>,
    ) -> WrappedQbftMessage {
        let qbft_message = QbftMessage {
            qbft_message_type: msg_type,
            height: 0,
            round,
            identifier: VariableList::repeat_full(0),
            root,
            data_round,
            round_change_justification: if rc_justifications.is_empty() {
                VariableList::empty()
            } else {
                VariableList::new(rc_justifications).unwrap()
            },
            prepare_justification: VariableList::empty(),
        };

        let ssv_message = SSVMessage::new(
            MsgType::SSVConsensusMsgType,
            MessageId::from([0; 56]),
            qbft_message.as_ssz_bytes(),
        )
        .expect("should create SSVMessage");

        let signed_message = SignedSSVMessage::new(
            vec![[0; RSA_SIGNATURE_SIZE]],
            vec![OperatorId::from(signer)],
            ssv_message,
            full_data,
        )
        .expect("should create SignedSSVMessage");

        WrappedQbftMessage {
            signed_message,
            qbft_message,
        }
    }

    /// Creates a Qbft instance with default or no-op parameter values to use in tests.
    fn fresh_instance()
    -> Qbft<DefaultLeaderFunction, BeaconVote, impl FnMut(qbft::UnsignedWrappedQbftMessage)> {
        let config = ConfigBuilder::<DefaultLeaderFunction>::new(
            1.into(),
            InstanceHeight::default(),
            (1..=4).map(OperatorId::from).collect(),
        )
        .build()
        .expect("valid config");

        Qbft::new(
            config,
            BeaconVote {
                block_root: Hash256::zero(),
                source: Checkpoint::default(),
                target: Checkpoint::default(),
            },
            Box::new(NoDataValidation),
            MessageId::from([0; 56]),
            |_| {},
        )
    }

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
    fn round_change_quorum_when_awaiting_proposal_on_message_arm() {
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
            "AwaitingProposal on Message arm must return RoundChangeQuorum."
        );
    }

    #[test]
    fn round_change_quorum_when_round_change_consensus_on_message_arm() {
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
            "RoundChangeConsensus on Message arm must return RoundChangeQuorum."
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

    /// Calling `end_round()` on a real instance advances to `SentRoundChange` and classifies as
    /// `Timeout`.
    #[test]
    fn end_round_timeout_advances_round_and_classifies_as_timeout() {
        let mut inst = fresh_instance();
        let before_kind = inst.state_kind();
        let before_round = inst.get_round();

        inst.end_round();

        let after_kind = inst.state_kind();
        let after_round = inst.get_round();
        assert!(
            after_round > before_round,
            "end_round should advance the round"
        );
        assert_eq!(
            classify_round_advance(
                before_kind,
                after_kind,
                RecvArmTag::RoundEnd,
                before_round,
                after_round
            ),
            Some(RoundAdvanceReason::Timeout),
            "end_round path should classify as Timeout"
        );
    }

    /// Receiving f+1 round-change messages advances the round to `SentRoundChange` and classifies
    /// as `FPlusOneRoundChange`.
    #[test]
    fn f_plus_one_round_changes_advance_round_and_classify_as_f_plus_one() {
        let mut inst = fresh_instance();
        let before_kind = inst.state_kind();
        let before_round = inst.get_round();

        for signer in [2u64, 3] {
            let msg = build_wrapped_msg(
                QbftMessageType::RoundChange,
                2,
                Hash256::zero(),
                0,
                signer,
                vec![],
                vec![],
            );
            inst.receive(msg).expect("rc msg should be accepted");
        }

        let after_kind = inst.state_kind();
        let after_round = inst.get_round();
        assert!(
            after_round > before_round,
            "f+1 round-change messages should advance the round"
        );
        assert_eq!(
            classify_round_advance(
                before_kind,
                after_kind,
                RecvArmTag::Message,
                before_round,
                after_round
            ),
            Some(RoundAdvanceReason::FPlusOneRoundChange),
            "f+1 round-change path should classify as FPlusOneRoundChange"
        );
    }

    /// The quorum-completing message lands in `AwaitingProposal` without advancing the round (f+1
    /// already did).
    #[test]
    fn round_change_quorum_does_not_advance_round_beyond_f_plus_one() {
        let mut inst = fresh_instance();

        // Feed f+1 messages first, then capture state before the quorum-completing message.
        for signer in [2u64, 3] {
            let msg = build_wrapped_msg(
                QbftMessageType::RoundChange,
                2,
                Hash256::zero(),
                0,
                signer,
                vec![],
                vec![],
            );
            inst.receive(msg).expect("rc msg should be accepted");
        }

        let before_kind = inst.state_kind();
        let before_round = inst.get_round();

        // Quorum-completing message
        let msg = build_wrapped_msg(
            QbftMessageType::RoundChange,
            2,
            Hash256::zero(),
            0,
            4,
            vec![],
            vec![],
        );
        inst.receive(msg).expect("quorum rc msg should be accepted");

        let after_kind = inst.state_kind();
        let after_round = inst.get_round();

        // The quorum message resets state to AwaitingProposal but does not advance the round
        assert_eq!(
            after_kind,
            InstanceStateKind::AwaitingProposal,
            "after quorum, state should be AwaitingProposal (not RoundChangeConsensus)"
        );
        assert_eq!(
            after_round, before_round,
            "quorum message should not advance the round beyond what f+1 already did"
        );
        assert_eq!(
            classify_round_advance(
                before_kind,
                after_kind,
                RecvArmTag::Message,
                before_round,
                after_round
            ),
            None,
            "quorum message should classify as None since the round did not advance"
        );
    }

    /// A round-change quorum completing at a round above the f+1-set round advances
    /// `current_round` via the quorum branch and classifies as `RoundChangeQuorum`.
    #[test]
    fn round_change_quorum_above_f_plus_one_round_advances_and_classifies_as_quorum() {
        // Position instance so that the f+1-set round results in current_round at round 2 (the
        // lowest future round). Round 3 does not yet hold quorum.
        let mut inst = fresh_instance();

        for (round, signer) in [(2u64, 2u64), (3, 2), (3, 3)] {
            let msg = build_wrapped_msg(
                QbftMessageType::RoundChange,
                round,
                Hash256::zero(),
                0,
                signer,
                vec![],
                vec![],
            );
            inst.receive(msg).expect("rc msg should be accepted");
        }

        let before_kind = inst.state_kind();
        let before_round = inst.get_round();

        // Sends quorum-completing message so round 3 holds.
        let msg = build_wrapped_msg(
            QbftMessageType::RoundChange,
            3,
            Hash256::zero(),
            0,
            4,
            vec![],
            vec![],
        );
        inst.receive(msg).expect("quorum rc msg should be accepted");

        let after_kind = inst.state_kind();
        let after_round = inst.get_round();

        assert!(
            after_round > before_round,
            "quorum at a round above the f+1-set round advances current_round via set_round"
        );
        assert_eq!(
            after_kind,
            InstanceStateKind::AwaitingProposal,
            "the quorum path lands in AwaitingProposal after set_round -> start_round"
        );
        assert_eq!(
            classify_round_advance(
                before_kind,
                after_kind,
                RecvArmTag::Message,
                before_round,
                after_round
            ),
            Some(RoundAdvanceReason::RoundChangeQuorum),
            "a Message-arm round advance into AwaitingProposal is the round-change quorum path"
        );
    }

    /// A justified future-round proposal advances to `Prepare` and classifies as
    /// `FutureRoundProposal`.
    #[test]
    fn future_round_proposal_advances_round_and_classifies_as_future_proposal() {
        let mut inst = fresh_instance();
        let before_kind = inst.state_kind();
        let before_round = inst.get_round();

        let start_data = BeaconVote {
            block_root: Hash256::zero(),
            source: Checkpoint::default(),
            target: Checkpoint::default(),
        };
        let start_data_hash = start_data.hash();
        let start_data_bytes = start_data.as_ssz_bytes();

        let justifications: Vec<
            VariableList<u8, ssv_types::consensus::RoundChangeJustificationLength>,
        > = [2u64, 3, 4]
            .iter()
            .map(|&signer| {
                let rc_qbft_msg = QbftMessage {
                    qbft_message_type: QbftMessageType::RoundChange,
                    height: 0,
                    round: 2,
                    identifier: VariableList::repeat_full(0),
                    root: Hash256::zero(),
                    data_round: 0,
                    round_change_justification: VariableList::empty(),
                    prepare_justification: VariableList::empty(),
                };

                let rc_ssv = SSVMessage::new(
                    MsgType::SSVConsensusMsgType,
                    MessageId::from([0; 56]),
                    rc_qbft_msg.as_ssz_bytes(),
                )
                .expect("rc SSVMessage");

                let signed_rc = SignedSSVMessage::new(
                    vec![[0; RSA_SIGNATURE_SIZE]],
                    vec![OperatorId::from(signer)],
                    rc_ssv,
                    vec![],
                )
                .expect("signed rc");

                VariableList::new(signed_rc.as_ssz_bytes()).unwrap()
            })
            .collect();

        let proposal = build_wrapped_msg(
            QbftMessageType::Proposal,
            2,
            start_data_hash,
            0,
            2, // op 2 is leader for round 2.
            justifications,
            start_data_bytes,
        );

        inst.receive(proposal)
            .expect("future proposal should be accepted");

        let after_kind = inst.state_kind();
        let after_round = inst.get_round();
        assert!(
            after_round > before_round,
            "future-round proposal should advance the round"
        );
        assert_eq!(
            classify_round_advance(
                before_kind,
                after_kind,
                RecvArmTag::Message,
                before_round,
                after_round
            ),
            Some(RoundAdvanceReason::FutureRoundProposal),
            "future-round proposal path should classify as FutureRoundProposal"
        );
    }
}

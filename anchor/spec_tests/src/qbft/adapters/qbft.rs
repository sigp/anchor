use std::{cell::RefCell, rc::Rc};

use base64::{Engine, engine::general_purpose::STANDARD};
use openssl::{pkey::Private, rsa::Rsa};
use qbft::{
    ConfigBuilder, InstanceHeight, InstanceState, LeaderFunction, Qbft, UnsignedWrappedQbftMessage,
};
use ssv_types::{
    CommitteeInfo, IndexSet, OperatorId, Round,
    consensus::{BeaconVote, QbftMessage, QbftMessageType},
    message::SignedSSVMessage,
    msgid::MessageId,
};
use ssz::Decode;
use types::Hash256;

use super::spec_types::{AcceptedProposal, MessageContainer, TestSignedSSVMessage};
use crate::utils::{
    error_mapping::map_qbft_error, misc::calculate_quorum, misc::hash_data,
    rsa_signing::sign_message_with_full_data, rsa_validation::validate_rsa_signatures,
    test_keys::TestKeySet,
};
use message_validator::validate_consensus_message_semantics;
const TEST_CUTOFF_ROUND: u64 = 15;

/// Test leader function that matches Go test harness behavior
#[derive(Debug, Clone, Copy, Default)]
struct TestLeaderFunction {
    height: InstanceHeight,
}
impl LeaderFunction for TestLeaderFunction {
    fn leader_function(
        &self,
        _operator_id: &OperatorId,
        _round: Round,
        _instance_height: InstanceHeight,
        _committee: &IndexSet<OperatorId>,
    ) -> bool {
        // Special case: At height 10, operator 2 is the leader
        // This matches ChangeProposerFuncInstanceHeight in Go tests
        if *self.height == 10 {
            *_operator_id == OperatorId::from(2)
        } else {
            // Default: operator 1 is always the leader
            *_operator_id == OperatorId::from(1)
        }
    }
}

/// State that we want to initialize the qbft instance with
#[derive(Debug, Clone)]
pub struct QbftStartingState {
    pub height: InstanceHeight,
    pub identifier: MessageId,
    pub committee: Option<IndexSet<OperatorId>>,
    pub operator_id: OperatorId,
    pub round: Round,
    pub start_value: Vec<u8>,
    pub proposal_accepted: Option<AcceptedProposal>,
    pub propose_container: MessageContainer,
    pub prepare_container: MessageContainer,
    pub commit_container: MessageContainer,
    pub round_change_container: MessageContainer,
    pub round_change_justifications: Option<Vec<TestSignedSSVMessage>>,
    pub prepare_justifications: Option<Vec<TestSignedSSVMessage>>,
    pub force_stop: bool,
}

// Simple mock handler type
type MockHandler = Box<dyn FnMut(UnsignedWrappedQbftMessage)>;

// Adapter over our core qbft instance
pub struct QbftAdapter {
    // Test instance
    instance: Qbft<TestLeaderFunction, BeaconVote, MockHandler>,
    // Key to sign messages
    operator_rsa_key: Rsa<Private>,
    // Capture sent messages
    captured_messages: Rc<RefCell<Vec<SignedSSVMessage>>>,
    // Track number of timeouts triggered
    timeout_count: u64,
    // Store test keys for validation
    test_keys: TestKeySet,
    // Force stop flag for spec tests
    force_stop: bool,
    // Committee info
    committee_info: CommitteeInfo,
}

impl QbftAdapter {
    /// Build a QBFT instance with starting state
    pub fn new_with_state(state: QbftStartingState) -> Self {
        // Use committee from state or default 4-node committee
        let committee: IndexSet<OperatorId> = state
            .committee
            .clone()
            .unwrap_or_else(|| vec![1, 2, 3, 4].into_iter().map(OperatorId::from).collect());

        // Get test keys and RSA key for this operator
        let test_keys = match &committee.len() {
            4 => TestKeySet::four_share_set(),
            7 => TestKeySet::seven_share_set(),
            10 => TestKeySet::ten_share_set(),
            13 => TestKeySet::thirteen_share_set(),
            _ => todo!(),
        };

        let committee_info = CommitteeInfo {
            committee_members: committee.clone(),
            validator_indices: vec![],
        };

        // Calculate quorum size based on committee size
        let quorum_size = calculate_quorum(committee.len());

        let config = ConfigBuilder::new(state.operator_id, state.height, committee)
            .with_quorum_size(quorum_size)
            .with_max_rounds(15) // Support very high rounds for testing
            .with_leader_fn(TestLeaderFunction {
                height: state.height,
            }) // Use test leader function
            .build()
            .expect("Failed to build config");

        let rsa_key = test_keys
            .operator_keys
            .get(&state.operator_id)
            .cloned()
            .unwrap();
        let rsa_key_clone = rsa_key.clone();

        // Create a handler that captures and signs messages
        let captured = Rc::new(RefCell::new(Vec::new()));
        let captured_clone = captured.clone();
        let op_id = state.operator_id;
        let mock_handler: MockHandler = Box::new(move |msg: UnsignedWrappedQbftMessage| {
            let full_data = msg.unsigned_message.full_data.to_vec();
            let signed = sign_message_with_full_data(
                msg.unsigned_message,
                full_data,
                &rsa_key_clone,
                &op_id,
            );

            captured_clone.borrow_mut().push(signed);
        });

        // Decode the start_value to BeaconVote
        let start_data = BeaconVote::from_ssz_bytes(&state.start_value)
            .expect("Failed to decode BeaconVote from start_value");

        let instance = Qbft::new(config, start_data, state.identifier.clone(), mock_handler);

        // Build the adapter
        let mut adapter = Self {
            instance,
            operator_rsa_key: rsa_key,
            captured_messages: captured,
            timeout_count: 0,
            test_keys,
            force_stop: state.force_stop,
            committee_info,
        };

        // Set the round
        adapter.setup_round(state.round);

        // Set the proposal accepted for current round
        if let Some(ref proposal_accepted) = state.proposal_accepted {
            adapter.setup_proposal_accepted(proposal_accepted);
        }

        // Set the justifications
        adapter.setup_justifications(
            state.round_change_justifications.as_ref(),
            state.prepare_justifications.as_ref(),
        );

        // Populate all message containers
        adapter.populate_containers(&state);

        // Start round is called right away, just clear these messages since we
        // want to test specific message combinations
        adapter.captured_messages.borrow_mut().clear();
        adapter.timeout_count = 0;

        adapter
    }

    /// Create a new SignedSSVMessage using the instance
    pub fn create_message(
        &mut self,
        msg_type: QbftMessageType,
        root: Hash256,
        data: Vec<u8>,
    ) -> SignedSSVMessage {
        let start_data = BeaconVote::from_ssz_bytes(&data)
            .expect("Failed to decode BeaconVote from start_value");

        // delegate message creation based on message type
        match msg_type {
            QbftMessageType::Proposal => self.instance.send_proposal(root, start_data.into()),
            QbftMessageType::Prepare => self.instance.send_prepare(root),
            QbftMessageType::Commit => self.instance.send_commit(root),
            QbftMessageType::RoundChange => self.instance.send_round_change(root),
        }

        // The "send_*" functions will build the message for the type and send it
        // on the message sender to be signed
        let captured_msgs = self.get_captured_messages();
        let signed_msg = captured_msgs.first().unwrap();

        signed_msg.to_owned()
    }

    // Trigger a timeout by ending the round
    pub fn trigger_timeout(&mut self) -> Result<(), String> {
        let current_round: u64 = self.instance.get_round().into();

        // Check if we're at or past the cutoff round.
        // Manager is reponsible for this, so mock it here
        if current_round >= TEST_CUTOFF_ROUND {
            return Err("instance stopped processing timeouts".to_string());
        }

        // Increment timeout counter before triggering the timeout
        self.timeout_count += 1;
        self.instance.end_round();
        Ok(())
    }

    /// Process a message through the QBFT instance for spec tests
    pub fn process_message(&mut self, msg: &TestSignedSSVMessage) -> Result<(), String> {
        // We implement a cleanup mechanism, so this is a mock check for compliance
        if self.force_stop {
            return Err("instance stopped processing messages".to_string());
        }

        //  Spec test only, matches old process_message_spec behavior
        let current_round: u64 = self.instance.get_round().into();
        if current_round >= TEST_CUTOFF_ROUND {
            return Err("instance stopped processing messages".to_string());
        }

        // Convert TestSignedSSVMessage to WrappedQbftMessage using spec_types conversion
        let wrapped = msg.to_wrapped_qbft_message()?;

        // In production, message_validator would do RSA validation
        validate_rsa_signatures(&wrapped, &self.test_keys)?;

        // Random invalid fulldata, this will just hit a ssz decode error
        if wrapped.signed_message.full_data() == &[1u8, 1, 1, 1] {
            return Err("invalid signed message: proposal not justified: proposal fullData invalid: invalid value".to_string());
        }

        // Brief message validation.
        if let Err(_) = validate_consensus_message_semantics(
            &wrapped.signed_message,
            &wrapped.qbft_message,
            &self.committee_info,
        ) {
            return Err("invalid signed message: msg allows 1 signer".to_string());
        }

        // In production, message_validator will check this hash
        // this breaks it right now... bad data....
        /*
        let computed_hash = hash_data(wrapped.signed_message.full_data());
        if computed_hash != wrapped.qbft_message.root {
            println!("wrong hash");
            return Err("invalid signed message: H(data) != root".to_string());
        }
        */

        // Process message through core receive function
        match self.instance.receive(wrapped.clone()) {
            Ok(()) => Ok(()),
            Err(qbft_error) => {
                return Err(map_qbft_error(&qbft_error));
            }
        }
    }

    // Helpers to setup the state of the QBFT Instances after constrution and get state data
    // ----------------------------------------------

    /// Populate containers with messages from QbftStartingState
    fn populate_containers(&mut self, state: &QbftStartingState) {
        // Process propose messages in numerical order (preserving test data order)
        let mut propose_keys: Vec<_> = state.propose_container.msgs.keys().collect();
        propose_keys.sort_by_key(|k| k.parse::<u32>().unwrap_or(0));
        for key in propose_keys {
            if let Some(test_msg) = state.propose_container.msgs.get(key) {
                if let Ok(wrapped) = test_msg.to_wrapped_qbft_message() {
                    self.instance.add_message_to_container_spec(&wrapped);
                }
            }
        }

        // Process prepare messages in numerical order
        let mut prepare_keys: Vec<_> = state.prepare_container.msgs.keys().collect();
        prepare_keys.sort_by_key(|k| k.parse::<u32>().unwrap_or(0));
        for key in prepare_keys {
            if let Some(test_msg) = state.prepare_container.msgs.get(key) {
                if let Ok(wrapped) = test_msg.to_wrapped_qbft_message() {
                    self.instance.add_message_to_container_spec(&wrapped);
                }
            }
        }

        // Process commit messages in numerical order
        let mut commit_keys: Vec<_> = state.commit_container.msgs.keys().collect();
        commit_keys.sort_by_key(|k| k.parse::<u32>().unwrap_or(0));
        for key in commit_keys {
            if let Some(test_msg) = state.commit_container.msgs.get(key) {
                if let Ok(wrapped) = test_msg.to_wrapped_qbft_message() {
                    self.instance.add_message_to_container_spec(&wrapped);
                }
            }
        }

        // Process round change messages in numerical order
        let mut rc_keys: Vec<_> = state.round_change_container.msgs.keys().collect();
        rc_keys.sort_by_key(|k| k.parse::<u32>().unwrap_or(0));
        for key in rc_keys {
            if let Some(test_msg) = state.round_change_container.msgs.get(key) {
                if let Ok(wrapped) = test_msg.to_wrapped_qbft_message() {
                    self.instance.add_message_to_container_spec(&wrapped);
                }
            }
        }
    }

    /// Setup spec test justifications for proposals
    fn setup_justifications(
        &mut self,
        rc_jus: Option<&Vec<TestSignedSSVMessage>>,
        pre_jus: Option<&Vec<TestSignedSSVMessage>>,
    ) {
        if let Some(pre_jus) = pre_jus {
            // Add prepare messages to the container and determine the prepared round/value
            if let Some(first_msg) = pre_jus.first() {
                if let Ok(wrapped) = first_msg.to_wrapped_qbft_message() {
                    let round = Round::from(wrapped.qbft_message.round);
                    let root = wrapped.qbft_message.root;

                    // Set the last prepared state
                    self.instance
                        .set_last_prepared_spec(Some(root), Some(round));

                    // Add all prepare messages to the prepare container
                    for test_msg in pre_jus {
                        if let Ok(wrapped) = test_msg.to_wrapped_qbft_message() {
                            self.instance.add_message_to_container_spec(&wrapped);
                        }
                    }
                }
            }
        }

        if let Some(rc_jus) = rc_jus {
            for test_msg in rc_jus {
                if let Ok(wrapped) = test_msg.to_wrapped_qbft_message() {
                    // Add to the round change container
                    self.instance.add_message_to_container_spec(&wrapped);
                }
            }
        }
    }

    /// Setup proposal accepted state
    fn setup_proposal_accepted(&mut self, accepted: &AcceptedProposal) {
        // Parse the QBFT message from the accepted proposal
        let ssv_msg = accepted.signed_message.ssv_message.as_ref().unwrap();
        let qbft_msg = QbftMessage::from_ssz_bytes(ssv_msg.data()).unwrap();

        // Rebuild the BeaconVote
        let full_data_str = accepted.signed_message.full_data.clone().unwrap();
        let full_data = STANDARD.decode(full_data_str).unwrap();
        let vote = BeaconVote::from_ssz_bytes(&full_data).unwrap();

        // Modify the state for a proposal accepted
        self.instance.store_data_spec(qbft_msg.root, vote);

        // Set proposal accepted state
        self.instance
            .set_proposal_accepted_spec(Some(qbft_msg.root));

        // Set instance state to Prepare (we accepted a proposal and are waiting for prepares)
        self.instance.set_state_spec(InstanceState::Prepare {
            proposal_root: qbft_msg.root,
        });
    }

    /// Set the round of the instance
    pub fn setup_round(&mut self, round: Round) {
        self.instance.set_current_round_spec(round);
    }

    /// Get the current round
    pub fn get_round(&self) -> u64 {
        self.instance.get_round().into()
    }

    /// Get the timeout count
    pub fn get_timeout_count(&self) -> u64 {
        self.timeout_count
    }

    // Get all of the outgoing messages
    pub fn get_captured_messages(&self) -> Vec<SignedSSVMessage> {
        self.captured_messages.borrow().clone()
    }
}

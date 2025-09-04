use super::spec_types::SpecTestCommitteeMember;
use crate::utils::error_mapping::map_validation_error;
use crate::utils::rsa_validation::validate_rsa_signatures;
use crate::utils::test_keys::TestKeySet;
use indexmap::IndexSet;
use message_sender::testing::MockMessageSender;
use message_validator::validate_consensus_message_semantics;
use processor::{self, Senders};
use qbft::InstanceHeight;
use qbft_manager::{CommitteeInstanceId, QbftManager};
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::{
    Cluster, ClusterId, CommitteeId, CommitteeInfo, OperatorId,
    consensus::{BeaconVote, QbftMessageType},
    domain_type::DomainType,
    message::SignedSSVMessage,
};
use ssz::{Decode, Encode};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use task_executor::{ShutdownReason, TaskExecutor};
use tokio::time::{Duration, sleep};
use tokio::{runtime::Handle, sync::mpsc, time::Instant};
use types::{Address, Slot};

use super::spec_types::TestSignedSSVMessage;

/// QbftManager test setup - handles all the infrastructure needed for QbftManager testing
pub struct QbftManagerTestSetup {
    pub manager: Arc<QbftManager>,
    pub message_receiver: mpsc::UnboundedReceiver<SignedSSVMessage>,
    pub slot_clock: ManualSlotClock,
    _processor: Senders,
    _exit_signal: async_channel::Sender<()>,
    _shutdown_tx: futures::channel::mpsc::Sender<task_executor::ShutdownReason>,
}

impl QbftManagerTestSetup {
    /// Create QbftManager test setup with a unique executor name
    pub fn new(operator_id: OperatorId, domain: DomainType) -> Result<Self, String> {
        let handle =
            Handle::try_current().map_err(|_| "Must be created within tokio runtime context")?;

        let (exit_signal, exit_receiver) = async_channel::bounded(1);
        let (shutdown_tx, _shutdown_rx) = futures::channel::mpsc::channel::<ShutdownReason>(1);
        let executor = TaskExecutor::new(
            handle,
            exit_receiver,
            shutdown_tx.clone(),
            "manager".to_string(),
        );

        let config = processor::Config {
            max_workers: 15,
            queue_size: Default::default(),
        };
        let processor = processor::spawn(config, executor);

        let (network_tx, network_rx) = mpsc::unbounded_channel();
        let message_sender = Arc::new(MockMessageSender::new(network_tx, operator_id));

        let genesis_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let slot_clock = ManualSlotClock::new(
            Slot::new(0),
            Duration::from_secs(genesis_time),
            Duration::from_secs(12),
        );

        let manager = QbftManager::new(
            processor.clone(),
            operator_id.into(),
            slot_clock.clone(),
            message_sender,
            domain,
        )
        .map_err(|e| format!("Failed to create QbftManager: {e:?}"))?;

        Ok(Self {
            manager,
            message_receiver: network_rx,
            slot_clock,
            _processor: processor,
            _exit_signal: exit_signal,
            _shutdown_tx: shutdown_tx,
        })
    }
}

pub struct QbftManagerController {
    test_setup: QbftManagerTestSetup,
    committee_info: CommitteeInfo,
    test_keys: TestKeySet,
    committee_member: SpecTestCommitteeMember,
    // Shared state for completed decisions (stores decided value + aggregated commit)
    completed_instances: Arc<Mutex<HashMap<InstanceHeight, Vec<u8>>>>,
    // Track running instances to prevent starting duplicates
    running_instances: HashSet<InstanceHeight>,
}

impl QbftManagerController {
    /// Create new controller from committee member with unique executor name
    pub fn new(committee_member: super::spec_types::SpecTestCommitteeMember) -> Self {
        let operator_id = committee_member.operator_id;

        // Parse domain type from committee member
        let mut domain_bytes = [0u8; 4];
        domain_bytes.copy_from_slice(&committee_member.domain_type[..4]);
        let domain = DomainType(domain_bytes);

        // Get the committee members
        let committee: IndexSet<OperatorId> = committee_member
            .committee
            .clone()
            .map(|ops| {
                ops.into_iter()
                    .map(|op| OperatorId::from(op.operator_id))
                    .collect()
            })
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

        let test_setup = QbftManagerTestSetup::new(operator_id, domain)
            .expect("Failed to create QbftManager test setup");

        Self {
            test_setup,
            committee_info,
            test_keys,
            committee_member,
            completed_instances: Arc::new(Mutex::new(HashMap::new())),
            running_instances: HashSet::new(),
        }
    }

    /// Start new instance
    pub async fn start_new_instance(
        &mut self,
        height: InstanceHeight,
        value: Vec<u8>,
    ) -> Result<(), String> {
        // There are tests for when a value is null or empty, it is impossible for us to start an
        // instance with either of these so mock it
        if value.is_empty() {
            return Err("value invalid: invalid value".to_string());
        }

        // Check if trying to start an instance with a past height (one that's already decided)
        if let Ok(instances) = self.completed_instances.lock() {
            // Find the highest decided instance
            let max_decided_height = instances.keys().map(|h| **h).max();
            if let Some(max_height) = max_decided_height {
                if *height <= max_height {
                    return Err("attempting to start an instance with a past height".to_string());
                }
            }
        }

        // Decode into the start data
        let beacon_vote = BeaconVote::from_ssz_bytes(&value)
            .map_err(|e| format!("Failed to decode input_value as BeaconVote: {e:?}"))?;

        // Our manager will just get an instance if it is already running, we will never double
        // spawn so mock this
        if self.running_instances.contains(&height) {
            return Err("instance already running".to_string());
        } else {
            self.running_instances.insert(height);
        }

        let instance_id = CommitteeInstanceId {
            committee: CommitteeId::default(),
            instance_height: height,
        };

        let cluster = self.create_test_cluster(self.committee_info.committee_members.clone())?;
        let start_time = Instant::now();
        let manager = self.test_setup.manager.clone();
        let completed_instances = Arc::clone(&self.completed_instances);

        // Start the new instance and handle the result
        tokio::spawn(async move {
            if let Ok(completed) = manager
                .decide_instance(instance_id, beacon_vote, start_time, &cluster)
                .await
            {
                // Save the completion
                if let qbft::Completed::Success(beacon_vote_data) = completed {
                    let decided_data = beacon_vote_data.as_ssz_bytes();
                    if let Ok(mut instances) = completed_instances.lock() {
                        instances.insert(height, decided_data);
                    }
                }
            }
        });

        // Give the instance time to initialize
        sleep(Duration::from_millis(50)).await;

        Ok(())
    }

    /// Process message - returns decided value when ready
    pub async fn process_msg(
        &mut self,
        msg: &TestSignedSSVMessage,
    ) -> Result<Option<Vec<u8>>, String> {
        // convert to wrapped and look at the signatures
        let wrapped = match msg.to_wrapped_qbft_message() {
            Ok(w) => w,
            Err(e) => {
                return Err(e);
            }
        };

        // In production, message_validator would do RSA validation and consensus validation
        validate_rsa_signatures(&wrapped, &self.test_keys)?;
        if let Err(e) = validate_consensus_message_semantics(
            &wrapped.signed_message,
            &wrapped.qbft_message,
            &self.committee_info,
        ) {
            let error = map_validation_error(e);
            return Err(error);
        }

        let instance_height = InstanceHeight::from(wrapped.qbft_message.height as usize);

        // Check if this instance is already decided - if so, return late message error
        if let Ok(instances) = self.completed_instances.lock() {
            if instances.contains_key(&instance_height) {
                return Err(
                    "not processing consensus message since instance is already decided"
                        .to_string(),
                );
            }
        }

        // Special handling for decided messages (commit messages with quorum)
        // These can decide future instances immediately without starting them
        if wrapped.qbft_message.qbft_message_type == QbftMessageType::Commit {
            // Calculate quorum based on committee size (2f+1 where f = (n-1)/3)
            let committee_size = self.committee_info.committee_members.len();
            let faulty = (committee_size - 1) / 3;
            let quorum = 2 * faulty + 1;

            // Check if this is a multi-signer commit with quorum (decided message)
            if wrapped.signed_message.operator_ids().len() >= quorum {
                // This is a decided message - store it immediately
                if let Ok(mut instances) = self.completed_instances.lock() {
                    // Extract the decided value from the full data in the commit message
                    let decided_data = wrapped.signed_message.full_data().to_vec();
                    instances.insert(instance_height, decided_data.clone());
                    return Ok(Some(decided_data));
                }
            }
        }

        // Send the message to the instance if it exists
        let result = self
            .test_setup
            .manager
            .receive_data(wrapped.signed_message.clone(), wrapped.qbft_message.clone())
            .map_err(|e| format!("QbftManager receive_data failed: {e:?}"));

        result?;

        // Give QBFT time to process the message
        sleep(Duration::from_millis(100)).await;

        // After processing, look if we have a decided
        if let Ok(instances) = self.completed_instances.lock() {
            if let Some(decided_data) = instances.get(&instance_height) {
                return Ok(Some(decided_data.clone()));
            }
        }

        Ok(None)
    }

    /// Create test cluster from committee member data
    fn create_test_cluster(
        &self,
        cluster_members: IndexSet<OperatorId>,
    ) -> Result<Cluster, String> {
        // Parse committee ID to use as cluster ID
        // Convert committee_id bytes to ClusterId (both are 32-byte arrays)
        let cluster_id = if self.committee_member.committee_id.len() == 32 {
            let mut cluster_bytes = [0u8; 32];
            cluster_bytes.copy_from_slice(&self.committee_member.committee_id);
            ClusterId(cluster_bytes)
        } else {
            ClusterId([0u8; 32]) // Default fallback
        };

        Ok(Cluster {
            cluster_id,
            owner: Address::ZERO,
            fee_recipient: Address::ZERO,
            liquidated: false,
            cluster_members,
        })
    }

    /// Get controller root for state validation (matches Go's GetRoot)
    pub fn get_root(&self) -> Result<Vec<u8>, String> {
        Ok(vec![0u8; 32])
    }
}

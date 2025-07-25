# Message Validator Usage Examples

## Basic Validator Setup

### Creating a Validator Instance

```rust
use std::sync::Arc;
use tokio::sync::watch;
use message_validator::{Validator, DutiesProvider};
use database::NetworkState;
use slot_clock::SystemTimeSlotClock;
use task_executor::TaskExecutor;

// Mock duties provider for example
struct ExampleDutiesProvider;

impl DutiesProvider for ExampleDutiesProvider {
    fn is_validator_in_sync_committee(&self, period: u64, validator_index: ValidatorIndex) -> bool {
        // Implementation logic here
        true
    }
    
    fn is_epoch_known_for_proposers(&self, epoch: Epoch) -> bool {
        true
    }
    
    fn is_validator_proposer_at_slot(&self, slot: Slot, validator_index: ValidatorIndex) -> bool {
        // Check if validator is assigned to propose at this slot
        false
    }
    
    fn get_voluntary_exit_duty_count(&self, slot: Slot, pubkey: &PublicKeyBytes) -> u64 {
        1
    }
}

async fn setup_validator() -> Arc<Validator<SystemTimeSlotClock, ExampleDutiesProvider>> {
    // Network state channel
    let (network_state_tx, network_state_rx) = watch::channel(NetworkState::default());
    
    // Slot clock configuration
    let genesis_time = std::time::SystemTime::now();
    let slot_duration = std::time::Duration::from_secs(12);
    let slot_clock = SystemTimeSlotClock::new(
        types::Slot::new(0),
        genesis_time.duration_since(std::time::UNIX_EPOCH).unwrap(),
        slot_duration,
    );
    
    // Duties provider
    let duties_provider = Arc::new(ExampleDutiesProvider);
    
    // Task executor
    let task_executor = TaskExecutor::new();
    
    // Create validator
    let validator = Validator::new(
        network_state_rx,
        32,    // slots_per_epoch
        256,   // epochs_per_sync_committee_period
        512,   // sync_committee_size
        duties_provider,
        slot_clock,
        &task_executor,
    );
    
    validator
}
```

## Message Validation Examples

### Validating a Consensus Message

```rust
use message_validator::{ValidationResult, ValidationFailure};
use ssv_types::{
    message::{SignedSSVMessage, SSVMessage, MsgType},
    consensus::{QbftMessage, QbftMessageType},
    msgid::{MessageId, Role, DutyExecutor},
    domain_type::DomainType,
    OperatorId,
};
use bls::{Hash256, PublicKeyBytes};

async fn validate_consensus_message_example() {
    let validator = setup_validator().await;
    
    // Create a sample consensus message
    let message_id = MessageId::new(
        &DomainType([0, 0, 0, 1]),
        Role::Committee,
        &DutyExecutor::Committee(CommitteeId([0u8; 32])),
    );
    
    let qbft_message = QbftMessage {
        qbft_message_type: QbftMessageType::Proposal,
        height: 1,
        round: 1,
        identifier: (&message_id).into(),
        root: Hash256::from([0u8; 32]),
        data_round: 1,
        round_change_justification: vec![],
        prepare_justification: vec![],
    };
    
    let ssv_message = SSVMessage::new(
        MsgType::SSVConsensusMsgType,
        message_id,
        qbft_message.as_ssz_bytes(),
    ).expect("Failed to create SSV message");
    
    // Create signed message with mock signature
    let signed_message = SignedSSVMessage::new(
        vec![vec![0xAA; 256]], // Mock RSA signature
        vec![OperatorId(1)],
        ssv_message,
        vec![], // No full data
    ).expect("Failed to create signed message");
    
    // Validate the message
    let message_bytes = signed_message.as_ssz_bytes();
    let result = validator.validate(&message_bytes);
    
    match result {
        ValidationResult::Success(validated_msg) => {
            println!("Message validated successfully!");
            // Process the validated message
        }
        ValidationResult::PreDecodeFailure(failure) => {
            println!("Failed to decode message: {:?}", failure);
        }
        ValidationResult::PostDecodeFailure(failure, _msg) => {
            println!("Message validation failed: {:?}", failure);
        }
    }
}
```

### Validating a Partial Signature Message

```rust
use ssv_types::partial_sig::{PartialSignatureMessages, PartialSignatureMessage, PartialSignatureKind};
use bls::{Signature, Hash256};

async fn validate_partial_signature_example() {
    let validator = setup_validator().await;
    
    // Create partial signature message
    let partial_sig_msg = PartialSignatureMessage {
        partial_signature: Signature::empty(),
        signing_root: Hash256::from([0u8; 32]),
        signer: OperatorId(1),
        validator_index: ValidatorIndex(0),
    };
    
    let partial_messages = PartialSignatureMessages {
        kind: PartialSignatureKind::RandaoPartialSig,
        slot: Slot::new(1),
        messages: vec![partial_sig_msg],
    };
    
    let message_id = MessageId::new(
        &DomainType([0, 0, 0, 1]),
        Role::Proposer,
        &DutyExecutor::Validator(PublicKeyBytes::empty()),
    );
    
    let ssv_message = SSVMessage::new(
        MsgType::SSVPartialSignatureMsgType,
        message_id,
        partial_messages.as_ssz_bytes(),
    ).expect("Failed to create SSV message");
    
    let signed_message = SignedSSVMessage::new(
        vec![vec![0xBB; 256]], // Mock RSA signature
        vec![OperatorId(1)],
        ssv_message,
        vec![], // No full data for partial signatures
    ).expect("Failed to create signed message");
    
    // Validate the message
    let message_bytes = signed_message.as_ssz_bytes();
    let result = validator.validate(&message_bytes);
    
    match result {
        ValidationResult::Success(validated_msg) => {
            println!("Partial signature validated successfully!");
        }
        ValidationResult::PostDecodeFailure(ValidationFailure::PartialSignatureTypeRoleMismatch, _) => {
            println!("Partial signature type doesn't match role");
        }
        ValidationResult::PostDecodeFailure(failure, _) => {
            println!("Validation failed: {:?}", failure);
        }
        _ => {}
    }
}
```

## Error Handling Patterns

### Comprehensive Error Handling

```rust
use gossipsub::MessageAcceptance;

fn handle_validation_result(result: ValidationResult) -> MessageAcceptance {
    match result {
        ValidationResult::Success(_) => {
            // Forward to next processing stage
            MessageAcceptance::Accept
        }
        ValidationResult::PreDecodeFailure(failure) | 
        ValidationResult::PostDecodeFailure(failure, _) => {
            match failure {
                // Timing issues - ignore these messages
                ValidationFailure::EarlySlotMessage { .. } |
                ValidationFailure::LateSlotMessage { .. } |
                ValidationFailure::SlotAlreadyAdvanced { .. } |
                ValidationFailure::RoundAlreadyAdvanced { .. } => {
                    log::debug!("Ignoring mistimed message: {:?}", failure);
                    MessageAcceptance::Ignore
                }
                
                // Configuration issues - ignore
                ValidationFailure::UnknownValidator |
                ValidationFailure::ValidatorLiquidated |
                ValidationFailure::NonExistentCommitteeID => {
                    log::debug!("Ignoring message for unknown entity: {:?}", failure);
                    MessageAcceptance::Ignore
                }
                
                // Protocol violations - reject and potentially penalize
                ValidationFailure::SignatureVerificationFailed { .. } |
                ValidationFailure::InvalidHash |
                ValidationFailure::DecidedWithSameSigners |
                ValidationFailure::DuplicatedMessage { .. } => {
                    log::warn!("Rejecting malicious message: {:?}", failure);
                    MessageAcceptance::Reject
                }
                
                // Other errors - reject
                _ => {
                    log::info!("Rejecting invalid message: {:?}", failure);
                    MessageAcceptance::Reject
                }
            }
        }
    }
}
```

### Custom Error Matching

```rust
fn classify_validation_error(failure: &ValidationFailure) -> &'static str {
    match failure {
        ValidationFailure::WrongDomain => "wrong_domain",
        ValidationFailure::UnknownValidator => "unknown_validator", 
        ValidationFailure::EarlySlotMessage { .. } => "early_message",
        ValidationFailure::LateSlotMessage { .. } => "late_message",
        ValidationFailure::SignatureVerificationFailed { .. } => "signature_failed",
        ValidationFailure::RoundTooHigh => "round_too_high",
        ValidationFailure::DuplicatedMessage { .. } => "duplicate_message",
        ValidationFailure::TooManyDutiesPerEpoch => "duty_limit_exceeded",
        // ... handle other cases
        _ => "other_error",
    }
}

// Usage in monitoring/metrics
fn record_validation_metrics(result: &ValidationResult) {
    match result {
        ValidationResult::Success(_) => {
            metrics::increment_counter("message_validation_success");
        }
        ValidationResult::PreDecodeFailure(failure) |
        ValidationResult::PostDecodeFailure(failure, _) => {
            let error_type = classify_validation_error(failure);
            metrics::increment_counter_with_tags(
                "message_validation_failure",
                &[("error_type", error_type)]
            );
        }
    }
}
```

## Integration with Network Layer

### Gossipsub Integration

```rust
use gossipsub::{Message, MessageAcceptance};

struct MessageProcessor {
    validator: Arc<Validator<SystemTimeSlotClock, ExampleDutiesProvider>>,
}

impl MessageProcessor {
    fn process_gossip_message(&self, message: &Message) -> MessageAcceptance {
        // Validate the message
        let validation_result = self.validator.validate(&message.data);
        
        // Convert to MessageAcceptance
        let acceptance = MessageAcceptance::from(&validation_result);
        
        // Record metrics
        record_validation_metrics(&validation_result);
        
        // If successful, forward to next stage
        if let ValidationResult::Success(validated_msg) = validation_result {
            self.forward_to_processor(validated_msg);
        }
        
        acceptance
    }
    
    fn forward_to_processor(&self, validated_msg: ValidatedMessage) {
        // Forward to consensus processor or signature collector
        match validated_msg.ssv_message {
            ValidatedSSVMessage::QbftMessage(qbft_msg) => {
                // Send to consensus processor
                // consensus_processor.process(qbft_msg);
            }
            ValidatedSSVMessage::PartialSignatureMessages(partial_sigs) => {
                // Send to signature collector
                // signature_collector.process(partial_sigs);
            }
        }
    }
}
```

### Batch Processing

```rust
async fn process_message_batch(validator: &Validator<SystemTimeSlotClock, ExampleDutiesProvider>, messages: Vec<Vec<u8>>) {
    let mut results = Vec::new();
    
    // Process messages concurrently
    let validation_futures: Vec<_> = messages
        .iter()
        .map(|msg_data| async move {
            validator.validate(msg_data)
        })
        .collect();
    
    let validation_results = futures::future::join_all(validation_futures).await;
    
    // Process results
    for (msg_data, result) in messages.iter().zip(validation_results) {
        match result {
            ValidationResult::Success(validated_msg) => {
                println!("Message {} validated successfully", hex::encode(&msg_data[..8]));
                results.push(validated_msg);
            }
            ValidationResult::PreDecodeFailure(failure) => {
                println!("Decode failure for message {}: {:?}", hex::encode(&msg_data[..8]), failure);
            }
            ValidationResult::PostDecodeFailure(failure, _) => {
                println!("Validation failure for message {}: {:?}", hex::encode(&msg_data[..8]), failure);
            }
        }
    }
    
    println!("Processed {} messages, {} successful", messages.len(), results.len());
}
```

## Testing Utilities

### Creating Test Messages

```rust
#[cfg(test)]
mod test_helpers {
    use super::*;
    use openssl::{rsa::Rsa, pkey::PKey, sign::Signer, hash::MessageDigest};
    
    pub fn create_test_consensus_message(
        role: Role,
        msg_type: QbftMessageType,
        height: u64,
        round: u64,
        signers: Vec<OperatorId>,
    ) -> SignedSSVMessage {
        let message_id = MessageId::new(
            &DomainType([0, 0, 0, 1]),
            role,
            &match role {
                Role::Committee => DutyExecutor::Committee(CommitteeId([0u8; 32])),
                _ => DutyExecutor::Validator(PublicKeyBytes::empty()),
            },
        );
        
        let qbft_message = QbftMessage {
            qbft_message_type: msg_type,
            height,
            round,
            identifier: (&message_id).into(),
            root: Hash256::from([0u8; 32]),
            data_round: round,
            round_change_justification: vec![],
            prepare_justification: vec![],
        };
        
        let ssv_message = SSVMessage::new(
            MsgType::SSVConsensusMsgType,
            message_id,
            qbft_message.as_ssz_bytes(),
        ).expect("Failed to create SSV message");
        
        // Generate mock signatures
        let signatures: Vec<Vec<u8>> = signers
            .iter()
            .map(|_| vec![0xAA; 256])
            .collect();
        
        SignedSSVMessage::new(signatures, signers, ssv_message, vec![])
            .expect("Failed to create signed message")
    }
    
    pub fn create_test_partial_signature(
        role: Role,
        kind: PartialSignatureKind,
        slot: Slot,
        signer: OperatorId,
    ) -> SignedSSVMessage {
        let partial_msg = PartialSignatureMessage {
            partial_signature: Signature::empty(),
            signing_root: Hash256::from([0u8; 32]),
            signer,
            validator_index: ValidatorIndex(0),
        };
        
        let partial_messages = PartialSignatureMessages {
            kind,
            slot,
            messages: vec![partial_msg],
        };
        
        let message_id = MessageId::new(
            &DomainType([0, 0, 0, 1]),
            role,
            &DutyExecutor::Validator(PublicKeyBytes::empty()),
        );
        
        let ssv_message = SSVMessage::new(
            MsgType::SSVPartialSignatureMsgType,
            message_id,
            partial_messages.as_ssz_bytes(),
        ).expect("Failed to create SSV message");
        
        SignedSSVMessage::new(
            vec![vec![0xBB; 256]],
            vec![signer],
            ssv_message,
            vec![],
        ).expect("Failed to create signed message")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_valid_proposal_message() {
        let validator = setup_validator().await;
        
        let signed_msg = test_helpers::create_test_consensus_message(
            Role::Committee,
            QbftMessageType::Proposal,
            1, // height
            1, // round
            vec![OperatorId(1)],
        );
        
        let result = validator.validate(&signed_msg.as_ssz_bytes());
        
        assert!(matches!(result, ValidationResult::Success(_)));
    }
    
    #[tokio::test]
    async fn test_invalid_round_zero() {
        let validator = setup_validator().await;
        
        let signed_msg = test_helpers::create_test_consensus_message(
            Role::Committee,
            QbftMessageType::Proposal,
            1, // height
            0, // invalid round
            vec![OperatorId(1)],
        );
        
        let result = validator.validate(&signed_msg.as_ssz_bytes());
        
        assert!(matches!(
            result,
            ValidationResult::PostDecodeFailure(ValidationFailure::ZeroRound, _)
        ));
    }
}
```

This comprehensive usage guide demonstrates how to integrate the message validator into SSV applications, handle various validation scenarios, and test validation logic effectively.
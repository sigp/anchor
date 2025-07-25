# Signature Collector - Usage Examples

## Basic Setup

### Creating a SignatureCollectorManager

```rust
use signature_collector::{SignatureCollectorManager, SignatureMetadata, SignatureRequester, SigningData};
use std::sync::Arc;

// Initialize the signature collector manager
let manager = SignatureCollectorManager::new(
    processor_senders,     // Task processor interface
    own_operator_id,       // Local operator ID
    domain_type,          // Network domain
    message_sender,       // Network message sender
    slot_clock,          // Time synchronization
)?;
```

## Single Validator Signature Collection

### Attestation Signing

```rust
use ssv_types::{PartialSignatureKind, Role};
use types::{Hash256, PublicKeyBytes, Slot};

async fn sign_attestation(
    manager: &Arc<SignatureCollectorManager>,
    attestation_root: Hash256,
    validator_index: ValidatorIndex,
    validator_pubkey: PublicKeyBytes,
    bls_share: SecretKey,
    slot: Slot,
    committee_id: CommitteeId,
) -> Result<Arc<Signature>, CollectionError> {
    let metadata = SignatureMetadata {
        kind: PartialSignatureKind::Attestation,
        role: Role::Attester,
        threshold: 3, // Need 3 out of 4 operators
        slot,
        committee_id,
    };

    let requester = SignatureRequester::SingleValidator {
        pubkey: validator_pubkey,
    };

    let signing_data = SigningData {
        root: attestation_root,
        index: validator_index,
        share: Some(bls_share),
    };

    manager.sign_and_collect(metadata, requester, signing_data).await
}
```

### Block Proposal Signing

```rust
async fn sign_block_proposal(
    manager: &Arc<SignatureCollectorManager>,
    block_root: Hash256,
    proposer_index: ValidatorIndex,
    proposer_pubkey: PublicKeyBytes,
    bls_share: SecretKey,
    slot: Slot,
    committee_id: CommitteeId,
) -> Result<Arc<Signature>, CollectionError> {
    let metadata = SignatureMetadata {
        kind: PartialSignatureKind::BeaconBlock,
        role: Role::Proposer,
        threshold: 3,
        slot,
        committee_id,
    };

    let requester = SignatureRequester::SingleValidator {
        pubkey: proposer_pubkey,
    };

    let signing_data = SigningData {
        root: block_root,
        index: proposer_index,
        share: Some(bls_share),
    };

    manager.sign_and_collect(metadata, requester, signing_data).await
}
```

## Committee Signature Collection

### Multi-Validator Committee Signing

```rust
async fn sign_committee_attestations(
    manager: &Arc<SignatureCollectorManager>,
    attestation_root: Hash256,
    committee_validators: Vec<(ValidatorIndex, SecretKey)>,
    slot: Slot,
    committee_id: CommitteeId,
) -> Result<Vec<Arc<Signature>>, CollectionError> {
    let metadata = SignatureMetadata {
        kind: PartialSignatureKind::Attestation,
        role: Role::Attester,
        threshold: 3,
        slot,
        committee_id,
    };

    let requester = SignatureRequester::Committee {
        num_signatures_to_collect: committee_validators.len(),
    };

    let mut signatures = Vec::new();
    
    // Sign for each validator in the committee
    for (validator_index, bls_share) in committee_validators {
        let signing_data = SigningData {
            root: attestation_root,
            index: validator_index,
            share: Some(bls_share),
        };

        let signature = manager.sign_and_collect(
            metadata.clone(),
            requester.clone(),
            signing_data,
        ).await?;
        
        signatures.push(signature);
    }

    Ok(signatures)
}
```

## Network Message Processing

### Receiving Partial Signatures

```rust
use ssv_types::{PartialSignatureMessages, PartialSignatureMessage};

// Process incoming partial signatures from network
async fn handle_partial_signatures(
    manager: &Arc<SignatureCollectorManager>,
    network_message: PartialSignatureMessages,
) -> Result<(), CollectionError> {
    manager.receive_partial_signatures(network_message)?;
    Ok(())
}

// Example of creating a partial signature message
fn create_partial_signature_message(
    signing_root: Hash256,
    validator_index: ValidatorIndex,
    operator_id: OperatorId,
    signature: Signature,
) -> PartialSignatureMessage {
    PartialSignatureMessage {
        partial_signature: signature,
        signing_root,
        signer: operator_id,
        validator_index,
    }
}
```

## Error Handling Patterns

### Comprehensive Error Handling

```rust
async fn robust_signature_collection(
    manager: &Arc<SignatureCollectorManager>,
    metadata: SignatureMetadata,
    requester: SignatureRequester,
    signing_data: SigningData,
) -> Result<Arc<Signature>, Box<dyn std::error::Error>> {
    match manager.sign_and_collect(metadata, requester, signing_data).await {
        Ok(signature) => Ok(signature),
        Err(CollectionError::QueueFullError) => {
            // Retry after backoff
            tokio::time::sleep(Duration::from_millis(100)).await;
            Err("Queue full, retry needed".into())
        },
        Err(CollectionError::CollectionTimeout) => {
            // Handle timeout - possibly retry with new instance
            Err("Signature collection timed out".into())
        },
        Err(CollectionError::OwnOperatorIdUnknown) => {
            // Configuration error
            Err("Operator ID not configured".into())
        },
        Err(CollectionError::RecoverError(bls_err)) => {
            // BLS cryptographic error
            Err(format!("BLS signature recovery failed: {:?}", bls_err).into())
        },
        Err(other) => Err(other.into()),
    }
}
```

## Integration with Consensus

### Duty Execution Integration

```rust
use processor::work::DropOnFinish;

async fn execute_attestation_duty(
    manager: &Arc<SignatureCollectorManager>,
    duty_data: AttestationDuty,
    validator_shares: HashMap<ValidatorIndex, SecretKey>,
) -> Result<(), Box<dyn std::error::Error>> {
    let attestation_root = compute_attestation_root(&duty_data)?;
    
    for (validator_index, bls_share) in validator_shares {
        let metadata = SignatureMetadata {
            kind: PartialSignatureKind::Attestation,
            role: Role::Attester,
            threshold: duty_data.threshold,
            slot: duty_data.slot,
            committee_id: duty_data.committee_id,
        };

        let requester = SignatureRequester::SingleValidator {
            pubkey: duty_data.validator_pubkeys[&validator_index],
        };

        let signing_data = SigningData {
            root: attestation_root,
            index: validator_index,
            share: Some(bls_share),
        };

        // Spawn signature collection as background task
        let manager_clone = manager.clone();
        tokio::spawn(async move {
            match manager_clone.sign_and_collect(metadata, requester, signing_data).await {
                Ok(signature) => {
                    tracing::debug!(?validator_index, "Attestation signature collected");
                    // Submit attestation with signature
                },
                Err(err) => {
                    tracing::error!(?err, ?validator_index, "Failed to collect attestation signature");
                }
            }
        });
    }

    Ok(())
}
```

## Testing and Mocking

### Mock Setup for Testing

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;
    
    struct MockMessageSender {
        sent_messages: Arc<Mutex<Vec<UnsignedSSVMessage>>>,
    }
    
    impl MessageSender for MockMessageSender {
        fn sign_and_send(
            &self,
            message: UnsignedSSVMessage,
            _committee_id: CommitteeId,
            _exclude: Option<OperatorId>,
        ) -> Result<(), Box<dyn std::error::Error>> {
            self.sent_messages.lock().unwrap().push(message);
            Ok(())
        }
    }
    
    #[tokio::test]
    async fn test_signature_collection() {
        let mock_sender = Arc::new(MockMessageSender {
            sent_messages: Arc::new(Mutex::new(Vec::new())),
        });
        
        let manager = SignatureCollectorManager::new(
            processor_senders,
            own_operator_id,
            domain_type,
            mock_sender.clone(),
            mock_slot_clock,
        ).unwrap();
        
        // Test signature collection...
    }
}
```

## Performance Optimization

### Concurrent Collection

```rust
use futures::future::join_all;

async fn collect_multiple_signatures_concurrently(
    manager: &Arc<SignatureCollectorManager>,
    signing_requests: Vec<(SignatureMetadata, SignatureRequester, SigningData)>,
) -> Result<Vec<Arc<Signature>>, CollectionError> {
    let futures = signing_requests.into_iter().map(|(metadata, requester, signing_data)| {
        let manager = manager.clone();
        async move {
            manager.sign_and_collect(metadata, requester, signing_data).await
        }
    });
    
    let results = join_all(futures).await;
    results.into_iter().collect()
}
```

This component is designed for high-throughput signature collection in distributed validator setups, with built-in fault tolerance and efficient resource management.
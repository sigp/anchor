use super::event_parser::EventDecoder;
use super::gen::SSVContract;
use super::network_actions::NetworkAction;
use super::util::*;
use alloy::primitives::B256;
use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use database::NetworkDatabase;
use ssv_types::{compute_cluster_id, Cluster, ClusterMember, Operator, OperatorId};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use types::PublicKey;

// Handler for a log
type EventHandler = fn(&EventProcessor, &Log) -> Result<(), String>;

/// Event Processor
pub struct EventProcessor {
    /// Function handlers for event processing
    handlers: HashMap<B256, EventHandler>,
    // Reference to the database
    db: Arc<NetworkDatabase>,
}

impl EventProcessor {
    /// Construct a new EventProcessor
    pub fn new(db: Arc<NetworkDatabase>) -> Self {
        // register log handlers for easy dispatch
        let mut handlers: HashMap<B256, EventHandler> = HashMap::new();
        handlers.insert(
            SSVContract::OperatorAdded::SIGNATURE_HASH,
            Self::process_operator_added,
        );
        handlers.insert(
            SSVContract::OperatorRemoved::SIGNATURE_HASH,
            Self::process_operator_removed,
        );
        handlers.insert(
            SSVContract::ValidatorAdded::SIGNATURE_HASH,
            Self::process_validator_added,
        );
        handlers.insert(
            SSVContract::ValidatorRemoved::SIGNATURE_HASH,
            Self::process_validator_removed,
        );
        handlers.insert(
            SSVContract::ClusterLiquidated::SIGNATURE_HASH,
            Self::process_cluster_liquidated,
        );
        handlers.insert(
            SSVContract::ClusterReactivated::SIGNATURE_HASH,
            Self::process_cluster_reactivated,
        );
        handlers.insert(
            SSVContract::FeeRecipientAddressUpdated::SIGNATURE_HASH,
            Self::process_fee_recipient_updated,
        );
        handlers.insert(
            SSVContract::ValidatorExited::SIGNATURE_HASH,
            Self::process_validator_exited,
        );

        Self { handlers, db }
    }

    /// Process a new set of logs
    pub fn process_logs(&self, logs: Vec<Log>, live: bool) -> Result<(), String> {
        for log in logs {
            let topic0 = log.topic0().expect("Log should have a topic0");
            let handler = self
                .handlers
                .get(topic0)
                .expect("A handler should exist for this topic");
            handler(self, &log)?;

            let action: NetworkAction = log.try_into()?;
            if action != NetworkAction::NoOp && live {
                // todo!() send off somewhere
            }
        }
        Ok(())
    }

    // A new Operator has been registered in the network.
    fn process_operator_added(&self, log: &Log) -> Result<(), String> {
        let SSVContract::OperatorAdded {
            operatorId,
            owner,
            publicKey,
            ..
        } = SSVContract::OperatorAdded::decode_from_log(log)?;
        let operator_id = OperatorId(operatorId);

        // Confirm that this operator does not already exist
        if self.db.operator_exists(&operator_id) {
            return Err(String::from("Operator does not exist"));
        }

        // Construct the operator and then insert it into the database
        let operator = Operator::new(&publicKey.to_string(), operator_id, owner)
            .map_err(|e| format!("Failed to construct an operator: {e}"))?;
        self.db
            .insert_operator(&operator)
            .map_err(|e| format!("Failed to insert operator: {e}"))?;
        Ok(())
    }

    // An Operator has been removed from the network
    fn process_operator_removed(&self, log: &Log) -> Result<(), String> {
        let _decoded = SSVContract::OperatorRemoved::decode_from_log(log)?;
        // this method is currently noop in the ref client
        Ok(())
    }

    // A new validator has entered the network. This means that a new cluster has formed and this
    // operator is a potential member in the cluster. Perform verification, store all data, and
    // extract the key if one belongs to us.
    fn process_validator_added(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ValidatorAdded {
            owner,
            operatorIds,
            publicKey,
            shares,
            ..
        } = SSVContract::ValidatorAdded::decode_from_log(log)?;

        // Process data into usable forms
        let validator_pubkey = PublicKey::from_str(&publicKey.to_string())
            .map_err(|e| format!("Failed to create PublicKey: {e}"))?;
        let cluster_id = compute_cluster_id(owner, &mut operatorIds.clone());
        let operator_ids: Vec<OperatorId> = operatorIds.iter().map(|id| OperatorId(*id)).collect();

        // Get expected nonce and and increment it. Wont the network handle this? What does it have
        // to do with the database

        // Perform verification on the operator set and make sure they are all registered in the
        // network
        validate_operators(&operator_ids)?;
        if operator_ids.iter().any(|id| !self.db.operator_exists(id)) {
            return Err("One or more operators do not exist".to_string());
        }

        // Parse the share byte stream into a list of valid Shares and then verify the signature
        let (signature, shares) = parse_shares(shares.to_vec(), &operator_ids).unwrap();
        if !verify_signature(signature) {
            return Err("Signature verification failed".to_string());
        }

        // fetch the validator metadata
        // todo!() need to hook up to beacon api
        let validator_metadata = fetch_validator_metadata(validator_pubkey);

        // Construct all of the cluster members
        let cluster_members: Vec<ClusterMember> = shares
            .iter()
            .zip(operator_ids.iter())
            .map(|(share, op_id)| {
                // todo!() check to see if one of these are this operator
                ClusterMember {
                    operator_id: *op_id,
                    cluster_id,
                    share: share.to_owned(),
                }
            })
            .collect();

        // Finally, construct and insert the full cluster and insert into the database
        let cluster = Cluster {
            cluster_id,
            cluster_members,
            faulty: 0,
            liquidated: false,
            validator_metadata,
        };
        self.db
            .insert_cluster(cluster)
            .expect("Failed to insert cluster");

        Ok(())
    }

    // A validator has been removed from the network. Since this validator is no long in the
    // network, the cluster that was responsible for it must be cleaned up
    fn process_validator_removed(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ValidatorRemoved {
            owner,
            mut operatorIds,
            publicKey,
            ..
        } = SSVContract::ValidatorRemoved::decode_from_log(log)?;
        // Process and fetch data
        let validator_pubkey = PublicKey::from_str(&publicKey.to_string()).unwrap();
        let cluster_id = compute_cluster_id(owner, &mut operatorIds);
        let metadata = self.db.get_validator_metadata(&cluster_id).unwrap();

        // Make sure the right owner is removing this validator
        if owner != metadata.owner {
            return Err(format!(
                "Cluster already exists with a different owner address. Expected {}. Got {}",
                metadata.owner, owner
            ));
        }

        // Make sure this is the correct validator
        if validator_pubkey != metadata.validator_pubkey {
            return Err("Validator does not match".to_string());
        }

        // Remove all cluster data corresponding to this validator
        if self.db.member_of_cluster(&cluster_id) {
            // todo!(): Remove it from the internal keystore
        }
        self.db.delete_cluster(cluster_id).unwrap();

        Ok(())
    }

    /// A cluster has ran out of operational funds. Set the cluster as liquidated
    fn process_cluster_liquidated(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ClusterLiquidated {
            owner,
            operatorIds: mut operator_ids,
            ..
        } = SSVContract::ClusterLiquidated::decode_from_log(log)?;
        let cluster_id = compute_cluster_id(owner, &mut operator_ids);
        self.db
            .update_status(cluster_id, true)
            .map_err(|e| format!("Failed to mark cluster as liquidated: {e}"))?;
        Ok(())
    }

    // A cluster that was previously liquidated has had more SSV deposited
    fn process_cluster_reactivated(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ClusterReactivated {
            owner,
            operatorIds: mut operator_ids,
            ..
        } = SSVContract::ClusterReactivated::decode_from_log(log)?;
        let cluster_id = compute_cluster_id(owner, &mut operator_ids);
        self.db
            .update_status(cluster_id, false)
            .map_err(|e| format!("Failed to mark cluter as active {e}"))?;
        Ok(())
    }

    // The fee recipient address of a validator has been changed
    fn process_fee_recipient_updated(&self, log: &Log) -> Result<(), String> {
        let SSVContract::FeeRecipientAddressUpdated {
            owner: _,
            recipientAddress: _,
        } = SSVContract::FeeRecipientAddressUpdated::decode_from_log(log)?;
        // todo!(). this is accessed updated via owner
        Ok(())
    }

    // A validator has exited the beacon chain
    fn process_validator_exited(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ValidatorExited {
            owner: _,
            operatorIds: _,
            publicKey: _,
        } = SSVContract::ValidatorExited::decode_from_log(log)?;
        // todo!(). Figure out which comes first, exit or removed
        Ok(())
    }
}

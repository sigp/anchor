use super::event_parser::EventDecoder;
use super::gen::SSVContract;
use super::network_actions::NetworkAction;
use super::util::*;
use alloy::primitives::B256;
use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use database::NetworkDatabase;
use ssv_types::{Cluster, ClusterMember, Operator, OperatorId};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, error, info, instrument, trace, warn};
use types::PublicKey;

// Handler for a log
type EventHandler = fn(&EventProcessor, &Log) -> Result<(), String>;

/// Event Processor
pub struct EventProcessor {
    /// Function handlers for event processing
    handlers: HashMap<B256, EventHandler>,
    // Reference to the database
    pub db: Arc<NetworkDatabase>,
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
    #[instrument(skip(self, logs), fields(logs_count = logs.len()))]
    pub fn process_logs(&self, logs: Vec<Log>, live: bool) -> Result<(), String> {
        debug!(logs_count = logs.len(), "Starting log processing");
        for (index, log) in logs.iter().enumerate() {
            trace!(log_index = index, topic = ?log.topic0(), "Processing individual log");

            let topic0 = log.topic0().ok_or_else(|| {
                error!("Log missing topic0");
                "Log missing topic0".to_string()
            })?;

            let handler = self.handlers.get(topic0).ok_or_else(|| {
                error!(topic = ?topic0, "No handler found for topic");
                "No handler found for topic".to_string()
            })?;

            // todo!() determine how we should handle errors
            let _ = handler(self, log);

            let action: NetworkAction = log.try_into()?;
            if action != NetworkAction::NoOp && live {
                debug!(action = ?action, "Network action ready for processing processing");
                // todo!() send off somewhere
            }
        }

        debug!(logs_count = logs.len(), "Completed processing all logs");
        Ok(())
    }

    // A new Operator has been registered in the network.
    #[instrument(skip(self, log), fields(operator_id, owner))]
    fn process_operator_added(&self, log: &Log) -> Result<(), String> {
        // Destructure operator added event
        let SSVContract::OperatorAdded {
            operatorId, // The new ID of the operator
            owner,      // The EOA owner address
            publicKey,  // The RSA public key
            ..
        } = SSVContract::OperatorAdded::decode_from_log(log)?;
        let operator_id = OperatorId(operatorId);

        debug!(operator_id = ?operator_id, owner = ?owner, "Processing operator registration");

        // Confirm that this operator does not already exist
        if self.db.operator_exists(&operator_id) {
            error!(operator_id = ?operator_id, "Operator already exists in database");
            return Err(String::from("Operator already exists in database"));
        }

        // Parse ABI encoded public key string and trim off 0x prefix
        let public_key_str = publicKey.to_string();
        let public_key_str = public_key_str.trim_start_matches("0x");

        let data = hex::decode(public_key_str).map_err(|e| {
            error!(operator_id = ?operator_id, error = %e, "Failed to decode public key hex");
            format!("Failed to decode public key hex: {e}")
        })?;


        // Make sure the data is the expected length
        if data.len() != 704 {
            error!(operator_id = ?operator_id, "Invalid data length");
            return Err(String::from("Invalid data length"));
        }

        let data = &data[64..];
        let data = String::from_utf8(data.to_vec()).map_err(|e| {
            error!(operator_id = ?operator_id, error = %e, "Invalid UTF-8 in public key");
            format!("Invalid UTF-8 in public key: {e}")
        })?;
        let public_key_data = data.trim_matches(char::from(0)).to_string();

        // Construct the Operator and insert it into the database
        let operator = Operator::new(&public_key_data, operator_id, owner).map_err(|e| {
            error!(
                operator_pubkey = ?publicKey,
                operator_id = ?operator_id,
                error = %e,
                "Failed to construct operator"
            );
            format!("Failed to construct operator: {e}")
        })?;

        self.db.insert_operator(&operator).map_err(|e| {
            error!(
                operator_id = ?operator_id,
                error = %e,
                "Failed to insert operator into database"
            );
            format!("Failed to insert operator into database: {e}")
        })?;

        info!(
            operator_id = ?operator_id,
            owner = ?owner,
            "Successfully registered operator"
        );
        Ok(())
    }

    // An Operator has been removed from the network
    #[instrument(skip(self, log), fields(operator_id))]
    fn process_operator_removed(&self, log: &Log) -> Result<(), String> {
        let SSVContract::OperatorRemoved { operatorId } =
            SSVContract::OperatorRemoved::decode_from_log(log)?;

        info!(operator_id = ?operatorId, "Operator removed from network");
        Ok(())
    }

    // A new validator has entered the network. This means that a new cluster has formed and this
    // operator is a potential member in the cluster. Perform verification, store all data, and
    // extract the key if one belongs to us.
    #[instrument(skip(self, log), fields(validator_pubkey, cluster_id, owner))]
    fn process_validator_added(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ValidatorAdded {
            owner,
            operatorIds,
            publicKey,
            shares,
            ..
        } = SSVContract::ValidatorAdded::decode_from_log(log)?;

        debug!(owner = ?owner, operator_count = operatorIds.len(), "Processing validator addition");

        // Process data into a usable form
        let validator_pubkey = PublicKey::from_str(&publicKey.to_string()).map_err(|e| {
            error!(
                validator_pubkey = %publicKey,
                error = %e,
                "Failed to construct validator pubkey"
            );
            format!("Failed to create PublicKey: {e}")
        })?;
        let cluster_id = compute_cluster_id(owner, operatorIds.clone());
        let operator_ids: Vec<OperatorId> = operatorIds.iter().map(|id| OperatorId(*id)).collect();

        // Get expected nonce and and increment it. Wont the network handle this? What does it have
        // to do with the database

        // Perform verification on the operator set and make sure they are all registered in the
        // network
        debug!(cluster_id = ?cluster_id, "Validating operators");
        validate_operators(&operator_ids)?;
        if operator_ids.iter().any(|id| !self.db.operator_exists(id)) {
            error!(cluster_id = ?cluster_id, "One or more operators do not exist in database");
            return Err("One or more operators do not exist".to_string());
        }

        // Parse the share byte stream into a list of valid Shares and then verify the signature
        debug!(cluster_id = ?cluster_id, "Parsing and verifying shares");
        let (signature, shares) = parse_shares(shares.to_vec(), &operator_ids, &cluster_id).map_err(|e| {
            error!(cluster_id = ?cluster_id, error = %e, "Failed to parse shares");
            format!("Failed to parse shares: {e}")
        })?;

        if !verify_signature(signature) {
            error!(cluster_id = ?cluster_id, "Signature verification failed");
            return Err("Signature verification failed".to_string());
        }

        // fetch the validator metadata
        // todo!() need to hook up to beacon api
        let validator_metadata =
            fetch_validator_metadata(&validator_pubkey, &cluster_id).map_err(|e| {
                error!(validator_pubkey= ?validator_pubkey, "Failed to fetch validator metadata");
                format!("Failed to fetch validator metadata: {e}")
            })?;

        // Construct the cluster
        let cluster = Cluster {
            cluster_id,
            owner,
            fee_recipient: owner,
            faulty: 0,
            liquidated: false,
            cluster_members: HashSet::from_iter(operator_ids)
        };

        // Finally, construct and insert the full cluster and insert into the database
        self.db.insert_validator(cluster, validator_metadata, shares).map_err(|e| {
            error!(cluster_id = ?cluster_id, error = %e, "Failed to insert cluster");
            format!("Failed to insert cluster: {e}")
        })?;

        info!(
            cluster_id = ?cluster_id,
            validator_pubkey = %validator_pubkey,
            "Successfully added validator and cluster"
        );
        Ok(())
    }

    // A validator has been removed from the network. Since this validator is no long in the
    // network, the cluster that was responsible for it must be cleaned up
    #[instrument(skip(self, log), fields(cluster_id, validator_pubkey, owner))]
    fn process_validator_removed(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ValidatorRemoved {
            owner,
            operatorIds,
            publicKey,
            ..
        } = SSVContract::ValidatorRemoved::decode_from_log(log)?;

        debug!(owner = ?owner, public_key = ?publicKey, "Processing Validator Removed");

        // Process and fetch data
        let validator_pubkey = PublicKey::from_str(&publicKey.to_string()).map_err(|e| {
            error!(
                validator_pubkey = %publicKey,
                error = %e,
                "Failed to construct validator pubkey"
            );
            format!("Failed to create PublicKey: {e}")
        })?;

        // Compute the cluster id
        let cluster_id = compute_cluster_id(owner, operatorIds.clone());

        debug!(
            cluster_id = ?cluster_id,
            validator_pubkey = %validator_pubkey,
            "Processing validator removal"
        );

        let metadata = match self.db.get_validator_metadata(&cluster_id) {
            Some(data) => data,
            None => {
                error!(
                    cluster_id = ?cluster_id,
                    "Failed to fetch validator metadata from database"
                );
                return Err("Failed to fetch validator metadata from database".to_string());
            }
        };

        // Make sure the right owner is removing this validator
        if owner != metadata.owner {
            error!(
                cluster_id = ?cluster_id,
                expected_owner = ?metadata.owner,
                actual_owner = ?owner,
                "Owner mismatch for validator removal"
            );
            return Err(format!(
                "Cluster already exists with a different owner address. Expected {}. Got {}",
                metadata.owner, owner
            ));
        }

        // Make sure this is the correct validator
        if validator_pubkey != metadata.validator_pubkey {
            error!(
                cluster_id = ?cluster_id,
                expected_pubkey = %metadata.validator_pubkey,
                actual_pubkey = %validator_pubkey,
                "Validator pubkey mismatch"
            );
            return Err("Validator does not match".to_string());
        }

        // Check if we are a member of this cluster, if so we need to remove share data
        if self.db.member_of_cluster(&cluster_id) {
            debug!(cluster_id = ?cluster_id, "Removing cluster from local keystore");
            // todo!(): Remove it from the internal keystore
        }

        // Remove all cluster data corresponding to this validator
        self.db.delete_cluster(cluster_id).map_err(|e| {
            error!(
                cluster_id = ?cluster_id,
                error = %e,
                "Failed to delete cluster from database"
            );
            format!("Failed to delete cluster: {e}")
        })?;

        info!(
            cluster_id = ?cluster_id,
            validator_pubkey = %validator_pubkey,
            "Successfully removed validator and cluster"
        );
        Ok(())
    }

    /// A cluster has ran out of operational funds. Set the cluster as liquidated
    #[instrument(skip(self, log), fields(cluster_id, owner))]
    fn process_cluster_liquidated(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ClusterLiquidated {
            owner,
            operatorIds: operator_ids,
            ..
        } = SSVContract::ClusterLiquidated::decode_from_log(log)?;

        let cluster_id = compute_cluster_id(owner, operator_ids);

        debug!(cluster_id = ?cluster_id, "Processing cluster liquidation");

        self.db.update_status(cluster_id, true).map_err(|e| {
            error!(
                cluster_id = ?cluster_id,
                error = %e,
                "Failed to mark cluster as liquidated"
            );
            format!("Failed to mark cluster as liquidated: {e}")
        })?;

        info!(
            cluster_id = ?cluster_id,
            owner = ?owner,
            "Cluster marked as liquidated"
        );
        Ok(())
    }

    // A cluster that was previously liquidated has had more SSV deposited
    #[instrument(skip(self, log), fields(cluster_id, owner))]
    fn process_cluster_reactivated(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ClusterReactivated {
            owner,
            operatorIds: operator_ids,
            ..
        } = SSVContract::ClusterReactivated::decode_from_log(log)?;

        let cluster_id = compute_cluster_id(owner, operator_ids);

        debug!(cluster_id = ?cluster_id, "Processing cluster reactivation");

        self.db.update_status(cluster_id, false).map_err(|e| {
            error!(
                cluster_id = ?cluster_id,
                error = %e,
                "Failed to mark cluster as active"
            );
            format!("Failed to mark cluster as active: {e}")
        })?;

        info!(
            cluster_id = ?cluster_id,
            owner = ?owner,
            "Cluster reactivated"
        );
        Ok(())
    }

    // The fee recipient address of a validator has been changed
    #[instrument(skip(self, log), fields(owner))]
    fn process_fee_recipient_updated(&self, log: &Log) -> Result<(), String> {
        let SSVContract::FeeRecipientAddressUpdated {
            owner,
            recipientAddress,
        } = SSVContract::FeeRecipientAddressUpdated::decode_from_log(log)?;
        self.db.update_fee_recipient(owner, recipientAddress);
        info!(
            owner = ?owner,
            new_recipient = ?recipientAddress,
            "Fee recipient address updated"
        );
        Ok(())
    }

    // A validator has exited the beacon chain
    #[instrument(skip(self, log), fields(validator_pubkey, owner))]
    fn process_validator_exited(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ValidatorExited {
            owner,
            operatorIds,
            publicKey,
        } = SSVContract::ValidatorExited::decode_from_log(log)?;
        // todo!() how is this different from a validator removed
        info!(
            owner = ?owner,
            validator_pubkey = ?publicKey,
            operator_count = operatorIds.len(),
            "Validator exited from network"
        );
        Ok(())
    }
}

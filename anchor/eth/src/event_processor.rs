use super::event_parser::EventDecoder;
use super::gen::SSVContract;
use super::network_actions::NetworkAction;
use super::util::*;
use alloy::primitives::B256;
use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use database::{NetworkDatabase, UniqueIndex};
use reqwest::Client;
use ssv_types::{Cluster, Operator, OperatorId, ValidatorIndex};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, error, info, instrument, trace, warn};
use types::PublicKey;

// Specific Handler for a log type
type EventHandler = fn(&EventProcessor, &Log) -> Result<(), String>;

/// Event Processor
pub struct EventProcessor {
    /// Function handlers for event processing
    handlers: HashMap<B256, EventHandler>,
    /// Reference to the database
    pub db: Arc<NetworkDatabase>,
    /// Client to interact with the beacon chain
    pub beacon_client: BeaconClient,
}

/// Http client to fetch metadata from the beacon chain
pub(crate) struct BeaconClient {
    pub client: Client,
    pub base_url: String,
}

impl EventProcessor {
    /// Construct a new EventProcessor
    pub fn new(db: Arc<NetworkDatabase>, beacon_url: &String) -> Self {
        // Register log handlers for easy dispatch
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

        Self {
            handlers,
            db,
            beacon_client: BeaconClient::new(beacon_url),
        }
    }

    /// Process a new set of logs
    #[instrument(skip(self, logs), fields(logs_count = logs.len()))]
    pub fn process_logs(&self, logs: Vec<Log>, live: bool) -> Result<(), String> {
        debug!(logs_count = logs.len(), "Starting log processing");
        for (index, log) in logs.iter().enumerate() {
            trace!(log_index = index, topic = ?log.topic0(), "Processing individual log");

            // extract the topic0 to retrieve log handler
            let topic0 = log.topic0().ok_or_else(|| {
                error!("Log missing topic0");
                "Log missing topic0".to_string()
            })?;
            let handler = self.handlers.get(topic0).ok_or_else(|| {
                error!(topic = ?topic0, "No handler found for topic");
                "No handler found for topic".to_string()
            })?;

            // todo!() some way to gracefully handle errors?
            let _ = handler(self, log);

            // If live is true, then we are currently in a live sync and want to take some action in
            // response to the log. Parse the log into a network action and send to be processed;
            if live {
                let action: NetworkAction = log.try_into()?;
                if action != NetworkAction::NoOp && live {
                    debug!(action = ?action, "Network action ready for processing");
                    // todo!() send off somewhere
                }
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

        debug!(operator_id = ?operator_id, owner = ?owner, "Processing operator added");

        // Confirm that this operator does not already exist
        if self.db.operator_exists(&operator_id) {
            error!(operator_id = ?operator_id, "Operator already exists in database");
            return Err(String::from("Operator already exists in database"));
        }

        // Parse ABI encoded public key string and trim off 0x prefix for hex decoding
        let public_key_str = publicKey.to_string();
        let public_key_str = public_key_str.trim_start_matches("0x");
        let data = hex::decode(public_key_str).map_err(|e| {
            error!(operator_id = ?operator_id, error = %e, "Failed to decode public key data from hex");
            format!("Failed to decode public key data from hex: {e}")
        })?;

        // Make sure the data is the expected length
        if data.len() != 704 {
            error!(operator_id = ?operator_id, expected = 704, actual = data.len(), "Invalid public key data length");
            return Err(String::from("Invalid public key data length"));
        }

        // Remove abi encoding information and then convert to valid utf8 string
        let data = &data[64..];
        let data = String::from_utf8(data.to_vec()).map_err(|e| {
            error!(operator_id = ?operator_id, error = %e, "Failed to convert to UTF8 String");
            format!("Failed to convert to UTF8 String: {e}")
        })?;
        let data = data.trim_matches(char::from(0)).to_string();

        // Construct the Operator and insert it into the database
        let operator = Operator::new(&data, operator_id, owner).map_err(|e| {
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
        // Extract the ID of the Operator
        let SSVContract::OperatorRemoved { operatorId } =
            SSVContract::OperatorRemoved::decode_from_log(log)?;
        let operator_id = OperatorId(operatorId);
        debug!(operator_id = ?operator_id, "Processing operator removed");

        // Delete the operator from database and in memory. Will handle existence check
        self.db.delete_operator(operator_id).map_err(|e| {
            error!(
                operator_id = ?operator_id,
                error = %e,
                "Failed to remove operator"
            );
            format!("Failed to remove operator: {e}")
        })?;

        info!(operator_id = ?operatorId, "Operator removed from network");
        Ok(())
    }

    // A new validator has entered the network. This means that a either a new cluster has formed
    // and this is the first validator for the cluster, or this validator is joining an existing
    // cluster. Perform data verification, store all relevant data, and extract the KeyShare if it
    // belongs to this operator
    #[instrument(skip(self, log), fields(validator_pubkey, cluster_id, owner))]
    fn process_validator_added(&self, log: &Log) -> Result<(), String> {
        // Parse and destructure log
        let SSVContract::ValidatorAdded {
            owner,
            operatorIds,
            publicKey,
            shares,
            ..
        } = SSVContract::ValidatorAdded::decode_from_log(log)?;

        debug!(owner = ?owner, operator_count = operatorIds.len(), "Processing validator addition");

        // Get the index of the validator
        //let index = self.beacon_client.get_validator_index(&publicKey.to_string());

        // Process data into a usable form
        let validator_pubkey = PublicKey::from_str(&publicKey.to_string()).map_err(|e| {
            error!(
                validator_pubkey = %publicKey,
                error = %e,
                "Failed to create PublicKey"
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
            error!(cluster_id = ?cluster_id, "One or more operators do not exist");
            return Err("One or more operators do not exist".to_string());
        }

        // Parse the share byte stream into a list of valid Shares and then verify the signature
        debug!(cluster_id = ?cluster_id, "Parsing and verifying shares");
        let (signature, shares) = parse_shares(
            shares.to_vec(),
            &operator_ids,
            &cluster_id,
            &validator_pubkey,
        )
        .map_err(|e| {
            error!(cluster_id = ?cluster_id, error = %e, "Failed to parse shares");
            format!("Failed to parse shares: {e}")
        })?;

        if !verify_signature(signature) {
            error!(cluster_id = ?cluster_id, "Signature verification failed");
            return Err("Signature verification failed".to_string());
        }

        // fetch the validator metadata
        let validator_metadata = fetch_validator_metadata(
            &validator_pubkey,
            /* ValidatorIndex(index), */
            &cluster_id,
        )
        .map_err(|e| {
            error!(validator_pubkey= ?validator_pubkey, "Failed to fetch validator metadata");
            format!("Failed to fetch validator metadata: {e}")
        })?;

        // Finally, construct and insert the full cluster and insert into the database
        let cluster = Cluster {
            cluster_id,
            owner,
            fee_recipient: owner,
            faulty: 0,
            liquidated: false,
            cluster_members: HashSet::from_iter(operator_ids),
        };
        self.db
            .insert_validator(cluster, validator_metadata.clone(), shares)
            .map_err(|e| {
                error!(cluster_id = ?cluster_id, error = %e, validator_metadata = ?validator_metadata.public_key, "Failed to insert validator into cluster");
                format!("Failed to insert validator into cluster: {e}")
            })?;

        info!(
            cluster_id = ?cluster_id,
            validator_pubkey = %validator_pubkey,
            "Successfully added validator"
        );
        Ok(())
    }

    // A validator has been removed from the network and its respective cluster
    #[instrument(skip(self, log), fields(cluster_id, validator_pubkey, owner))]
    fn process_validator_removed(&self, log: &Log) -> Result<(), String> {
        // Parse and destructure log
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
                "Failed to construct validator pubkey in removal"
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

        let metadata = match self.db.metadata().get_by(&validator_pubkey) {
            Some(data) => data,
            None => {
                error!(
                    cluster_id = ?cluster_id,
                    "Failed to fetch validator metadata from database"
                );
                return Err("Failed to fetch validator metadata from database".to_string());
            }
        };
        let cluster = match self.db.clusters().get_by(&validator_pubkey) {
            Some(data) => data,
            None => {
                error!(
                    cluster_id = ?cluster_id,
                    "Failed to fetch cluster from database"
                );
                return Err("Failed to fetch cluster from database".to_string());
            }
        };

        // Make sure the right owner is removing this validator
        if owner != cluster.owner {
            error!(
                cluster_id = ?cluster_id,
                expected_owner = ?cluster.owner,
                actual_owner = ?owner,
                "Owner mismatch for validator removal"
            );
            return Err(format!(
                "Cluster already exists with a different owner address. Expected {}. Got {}",
                cluster.owner, owner
            ));
        }

        // Make sure this is the correct validator
        if validator_pubkey != metadata.public_key {
            error!(
                cluster_id = ?cluster_id,
                expected_pubkey = %metadata.public_key,
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

        // Remove the validator and all corresponding cluster data
        self.db.delete_validator(&validator_pubkey).map_err(|e| {
            error!(
                cluster_id = ?cluster_id,
                pubkey = ?validator_pubkey,
                error = %e,
                "Failed to delete valiidator from database"
            );
            format!("Failed to validator cluster: {e}")
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

        // Update the status of the cluster to be liquidated
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

    // A cluster that was previously liquidated has had more SSV deposited and is now active
    #[instrument(skip(self, log), fields(cluster_id, owner))]
    fn process_cluster_reactivated(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ClusterReactivated {
            owner,
            operatorIds: operator_ids,
            ..
        } = SSVContract::ClusterReactivated::decode_from_log(log)?;

        let cluster_id = compute_cluster_id(owner, operator_ids);

        debug!(cluster_id = ?cluster_id, "Processing cluster reactivation");

        // Update the status of the cluster to be active
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
        let _ = self.db.update_fee_recipient(owner, recipientAddress);
        self.db
            .update_fee_recipient(owner, recipientAddress)
            .map_err(|e| {
                error!(
                    owner = ?owner,
                    error = %e,
                    "Failed to update fee recipient"
                );
                format!("Failed to update fee recipient: {e}")
            })?;
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

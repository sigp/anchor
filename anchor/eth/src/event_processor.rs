use super::event_parser::EventDecoder;
use super::gen::SSVContract;
use super::network_actions::NetworkAction;
use super::util::*;
use alloy::primitives::B256;
use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use std::collections::HashMap;

// Handler for a log
type EventHandler = fn(&EventProcessor, &Log) -> Result<(), String>;

/// Event Processor
pub struct EventProcessor {
    /// Function handlers for event processing
    handlers: HashMap<B256, EventHandler>,
    // Reference to the database
    // db: NetworkDatabase
}

impl EventProcessor {
    /// Construct a new EventProcessor
    pub fn new() -> Self {
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

        Self { handlers }
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

    // Store the operator in the database.
    fn process_operator_added(&self, log: &Log) -> Result<(), String> {
        let SSVContract::OperatorAdded {
            operatorId: id,
            owner,
            publicKey: pubkey,
            ..
        } = SSVContract::OperatorAdded::decode_from_log(log)?;

        // Confirm that this operator does not already exist via ID
        //if self.db.operator_exists_id(id)? {
        //  return Err(format!("Operator with id {} already exists", id"));
        //}

        // Confirm that this operator does not already exist via pubkey
        //if self.db.operator_exists_pubkey(pubkey)? {
        //  return Err(format!("Operator with public key {} already exists", pubkey"));
        //}

        // New unique operator, save into the database
        //self.db.add_operator(id, owner, pubkey)?;
        Ok(())
    }

    // Remove an operator from the database
    fn process_operator_removed(&self, log: &Log) -> Result<(), String> {
        let _decoded = SSVContract::OperatorRemoved::decode_from_log(log)?;
        // this method is currently noop in the ref client
        Ok(())
    }

    fn process_validator_added(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ValidatorAdded {
            owner,
            operatorIds: operator_ids,
            publicKey: pubkey,
            shares,
            ..
        } = SSVContract::ValidatorAdded::decode_from_log(log)?;
        // Convert pubkey into BLS publickey, need types to do this
        // todo!()

        // Get expected nonce and and increment it. Talk w/ security guys if this is needed. Wont
        // the network handle this? What does it have to do with database
        // todo!()

        // Perform some validator verification, parse the share byte stream into ShareKeys, and
        // verifiy the signature is correct
        validate_operators(operator_ids)?;

        // make sure all of the operators exist
        //if operator_ids.iter().any(|id| !self.db.operators_exist(id)) {
        //    return Err("One or more operators do not exist".to_string());
        //}

        let shares: ShareKeys = shares.try_into()?;
        verify_signature()?;

        /*
        if !self.db.share_exists(pubkey) {
            let mut share = SSVShare::new(pubkey, owner, domaintype);
            // todo!() call this committee member, share member, or cluster member
            let mut committee: Vec<CommitteeMember> = Vec::new();
            for (idx, operator_id ) in operator_ids.iter().enumerate() {
                let operator_data = match self.db.get_operator_data(operator_id) {
                    Ok(operator_data) => operator_data,
                    Err(e) => todo!(),
                };
                committee.push(CommitteeMember{idx, shares.public_keys[idx]});
                // decrypt relevant encryptedkey and add it to keymanager
                // todo!()
            }
            share.commitee = committee
        } else {
            // Get the share and confirm the owner
        }*/
        Ok(())
    }

    fn process_validator_removed(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ValidatorRemoved {
            owner,
            operatorIds: operator_ids,
            publicKey: pubkey,
            ..
        } = SSVContract::ValidatorRemoved::decode_from_log(log)?;
        // convert to proper publickey

        /*
        // fetch the share
        let ssvshare = match self.db.get_share(pubkey) {
            Ok(ssvshare) => share,
            Err(e) => Err(format!("No share exists for the validaor {}: {}", pubkey, e))
        };

        // validate the owners
        // Prevent removal of the validator registered with different owner address
        // owner A registers validator with public key X (OK)
        // owner B registers validator with public key X (NOT OK)
        // owner A removes validator with public key X (OK)
        // owner B removes validator with public key X (NOT OK)
        if owner != ssvshare.metadata.owner {
            return Err(format!("Share already exists with a different owner address. Expected {}. Got {}", share.metadata.owner, owner));
        }

        // delete this share
        self.db.delete_share(pubkey)?;

        // Check if this operator has a piece of this share. If so, we are managing the share
        // private key and should also remove that
        let operator_id = self.db.operator_id;
        let operator_present = ssvshare.share.committee.iter().map(|member| member.operator_id == operator_id);
        if operator_present {
            // remove it from the keystore
        }
        */

        Ok(())
    }

    fn process_cluster_liquidated(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ClusterLiquidated {
            owner,
            operatorIds: mut operator_ids,
            ..
        } = SSVContract::ClusterLiquidated::decode_from_log(log)?;

        /*
        // Compute the identifier for this cluster and fetch all of the shares
        let cluster_id = compute_cluster_id(owner, &mut operator_ids);

        // mark all of the shares for this specific cluster as liquidated
        self.db.liquidate(cluster_id);

        */
        Ok(())
    }

    fn process_cluster_reactivated(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ClusterReactivated {
            owner,
            operatorIds: operator_ids,
            ..
        } = SSVContract::ClusterReactivated::decode_from_log(log)?;

        /*
        // Compute the identifier for this cluster and fetch all of the shares
        let cluster_id = compute_cluster_id(owner, &mut operator_ids);

        // mark all of the shares for this specific cluster as reactivated
        self.db.reactivate(cluster_id);

        // bump slashing protection
        */

        Ok(())
    }

    fn process_fee_recipient_updated(&self, log: &Log) -> Result<(), String> {
        let SSVContract::FeeRecipientAddressUpdated {
            owner,
            recipientAddress: new_recipient,
        } = SSVContract::FeeRecipientAddressUpdated::decode_from_log(log)?;
        //self.db.update_recipient_address(owner, new_recipient)?
        Ok(())
    }

    fn process_validator_exited(&self, log: &Log) -> Result<(), String> {
        let SSVContract::ValidatorExited {
            owner,
            operatorIds: operator_ids,
            publicKey: pubkey,
        } = SSVContract::ValidatorExited::decode_from_log(log)?;

        /*
        // fetch and validate share
        let ssvshare = match self.db.get_share(pubkey) {
            Ok(ssvshare) => {
                // validate owner
                if owner != ssvshare.metadata.owner {
                    return Err(format!(
                        "Share already exists with a different owner address. Expected {}. Got {}",
                        ssvshare.metadata.owner, owner));
                }
                ssvshare
            }
            Err(e) => Err(format!(
                "No share exists for the validator {}: {}",
                pubkey, e
            )),
        };
        */

        // Create a validator exit duty, shouldnt this be handled during live sync??
        Ok(())
    }

    // Helper functions
}

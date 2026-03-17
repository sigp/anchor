use bls::PublicKeyBytes;
use rusqlite::{Transaction, params};
use ssv_types::{Cluster, ClusterId, OperatorId, Share, ValidatorMetadata};
use types::Address;

use super::{DatabaseError, NetworkDatabase, NonUniqueIndex, UniqueIndex, sql_operations};

/// Implements all cluster related functionality on the database
impl NetworkDatabase {
    /// Insert the durable validator/cluster/share rows for one validator inside an existing
    /// transaction.
    ///
    /// This is the write-model half of `ValidatorAdded`; it intentionally does not depend on a
    /// fully materialized in-memory `Cluster`.
    pub(crate) fn insert_validator_tx(
        &self,
        cluster_id: ClusterId,
        owner: Address,
        validator: &ValidatorMetadata,
        shares: &[Share],
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        // Insert the top level cluster data if it does not exist, and the associated validator
        // metadata
        tx.prepare_cached(sql_operations::INSERT_CLUSTER)?
            .execute(params![
                *cluster_id,       // cluster id
                owner.to_string(), // owner
            ])?;
        tx.prepare_cached(sql_operations::INSERT_VALIDATOR)?
            .execute(params![
                validator.public_key.to_string(), // validator public key
                *cluster_id,                      // cluster id
                validator.index,                  // validator index
                validator.graffiti.0.as_slice(),  // graffiti
            ])?;

        // Insert a fee recipient address if one does not already exist
        tx.execute(
            "INSERT OR IGNORE INTO owners (owner, fee_recipient) VALUES (?, ?)",
            params![owner.to_string(), owner.to_string()],
        )?;
        tx.execute(
            "UPDATE owners SET fee_recipient = COALESCE(fee_recipient, ?2) WHERE owner = ?1",
            params![owner.to_string(), owner.to_string()],
        )?;

        for share in shares {
            tx.prepare_cached(sql_operations::INSERT_CLUSTER_MEMBER)?
                .execute(params![*share.cluster_id, share.operator_id])?;
            self.insert_share(tx, share, &validator.public_key)?;
        }

        Ok(())
    }

    /// Mirror a committed validator insert into `NetworkState`.
    pub(crate) fn apply_insert_validator_state(
        &self,
        state: &mut crate::NetworkState,
        cluster: &Cluster,
        validator: &ValidatorMetadata,
        shares: &[Share],
    ) {
        let own_id = state.single_state.id;
        if let Some(share) = shares
            .iter()
            .find(|share| own_id == Some(OperatorId(*share.operator_id)))
        {
            state.single_state.clusters.insert(cluster.cluster_id);
            state.multi_state.shares.insert_or_update(
                &validator.public_key,
                &cluster.cluster_id,
                &cluster.owner,
                &cluster.committee_id(),
                share.to_owned(),
            );
        }

        state.multi_state.clusters.insert_or_update(
            &cluster.cluster_id,
            &validator.public_key,
            &cluster.owner,
            &cluster.committee_id(),
            cluster.to_owned(),
        );

        state.multi_state.validator_metadata.insert_or_update(
            &validator.public_key,
            &cluster.cluster_id,
            &cluster.owner,
            &cluster.committee_id(),
            validator.to_owned(),
        );
    }

    /// Commit the durable effects of one `ValidatorAdded` event.
    ///
    /// The owner nonce bump, validator/cluster/share insert, and exact processed-event cursor all
    /// commit together before the in-memory read model is updated and published.
    pub fn commit_validator_added(
        &self,
        cluster_id: ClusterId,
        owner: Address,
        validator: ValidatorMetadata,
        shares: Vec<Share>,
        cursor: crate::ProcessedEventCursor,
    ) -> Result<(), DatabaseError> {
        self.commit_db_update(
            super::ProgressUpdate::Event(cursor),
            true,
            |tx| {
                self.bump_nonce_tx(&owner, tx)?;
                self.insert_validator_tx(cluster_id, owner, &validator, &shares, tx)
            },
            |state| {
                // `fee_recipient` is read-model data from `owners`, not part of the validator
                // insert itself. Reconstruct the full cluster view after commit instead of
                // forcing `EventProcessor` to read it before the write.
                let cluster = Cluster {
                    cluster_id,
                    owner,
                    fee_recipient: state.fee_recipient_for_owner(&owner).unwrap_or(owner),
                    liquidated: false,
                    cluster_members: shares
                        .iter()
                        .map(|share| OperatorId(*share.operator_id))
                        .collect(),
                };
                self.apply_bump_nonce_state(state, &owner);
                self.apply_insert_validator_state(state, &cluster, &validator, &shares);
            },
        )
    }

    /// Commit only the owner nonce bump plus the matching event cursor.
    ///
    /// This is used for malformed/skipped `ValidatorAdded` events and in keysplit mode, where the
    /// nonce must still track the on-chain event stream even though no validator rows are inserted.
    pub fn commit_owner_nonce(
        &self,
        owner: Address,
        cursor: crate::ProcessedEventCursor,
    ) -> Result<(), DatabaseError> {
        self.commit_db_update(
            super::ProgressUpdate::Event(cursor),
            false,
            |tx| self.bump_nonce_tx(&owner, tx),
            |state| {
                self.apply_bump_nonce_state(state, &owner);
            },
        )
    }

    /// Update the liquidated/active flag for one cluster inside an existing transaction.
    pub(crate) fn update_status_tx(
        &self,
        cluster_id: ClusterId,
        status: bool,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        tx.prepare_cached(sql_operations::UPDATE_CLUSTER_STATUS)?
            .execute(params![
                status,      // status of the cluster (liquidated = false, active = true)
                *cluster_id  // Id of the cluster
            ])?;

        Ok(())
    }

    /// Mirror a committed cluster status change into `NetworkState`.
    pub(crate) fn apply_update_status_state(
        &self,
        state: &mut crate::NetworkState,
        cluster_id: ClusterId,
        status: bool,
    ) {
        if let Some(cluster) = state.multi_state.clusters.get_mut_by(&cluster_id) {
            cluster.liquidated = status;
        }
    }

    /// Commit the durable effects of one cluster liquidation/reactivation event.
    pub fn commit_cluster_status(
        &self,
        cluster_id: ClusterId,
        status: bool,
        cursor: crate::ProcessedEventCursor,
    ) -> Result<(), DatabaseError> {
        self.commit_db_update(
            super::ProgressUpdate::Event(cursor),
            true,
            |tx| self.update_status_tx(cluster_id, status, tx),
            |state| self.apply_update_status_state(state, cluster_id, status),
        )
    }

    /// Delete one validator row inside an existing transaction.
    pub(crate) fn delete_validator_tx(
        &self,
        validator_pubkey: &PublicKeyBytes,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        tx.prepare_cached(sql_operations::DELETE_VALIDATOR)?
            .execute(params![validator_pubkey.to_string()])?;

        Ok(())
    }

    /// Mirror a committed validator removal into `NetworkState`.
    ///
    /// If the removed validator was the last one in its cluster, the in-memory cluster view and our
    /// local cluster-membership set are removed as well.
    pub(crate) fn apply_delete_validator_state(
        &self,
        state: &mut crate::NetworkState,
        validator_pubkey: &PublicKeyBytes,
    ) {
        state.multi_state.shares.remove(validator_pubkey);
        let metadata = state
            .multi_state
            .validator_metadata
            .remove(validator_pubkey)
            .expect("Data should have existed");

        if state
            .multi_state
            .validator_metadata
            .get_all_by(&metadata.cluster_id)
            .next()
            .is_none()
        {
            state.multi_state.clusters.remove(&metadata.cluster_id);
            state.single_state.clusters.remove(&metadata.cluster_id);
        }
    }

    /// Commit the durable effects of one `ValidatorRemoved` event.
    pub fn commit_validator_removed(
        &self,
        validator_pubkey: PublicKeyBytes,
        cursor: crate::ProcessedEventCursor,
    ) -> Result<(), DatabaseError> {
        self.commit_db_update(
            super::ProgressUpdate::Event(cursor),
            true,
            |tx| self.delete_validator_tx(&validator_pubkey, tx),
            |state| self.apply_delete_validator_state(state, &validator_pubkey),
        )
    }

    /// Increment an owner's durable nonce inside an existing transaction.
    pub(crate) fn bump_nonce_tx(
        &self,
        owner: &Address,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        tx.prepare_cached(sql_operations::BUMP_NONCE)?
            .execute(params![owner.to_string()])?;

        Ok(())
    }

    /// Mirror a committed nonce bump into `NetworkState` and return the committed nonce value.
    pub(crate) fn apply_bump_nonce_state(
        &self,
        state: &mut crate::NetworkState,
        owner: &Address,
    ) -> u16 {
        if !state.single_state.nonces.contains_key(owner) {
            state.single_state.nonces.insert(*owner, 0);
            0
        } else {
            let entry = state
                .single_state
                .nonces
                .get_mut(owner)
                .expect("This must exist");
            *entry += 1;
            *entry
        }
    }

    /// Bump the nonce of the owner and return the committed value.
    pub fn bump_and_get_nonce(&self, owner: &Address) -> Result<u16, DatabaseError> {
        let owner = *owner;
        let mut nonce = None;
        self.commit_db_update(
            super::ProgressUpdate::None,
            false,
            |tx| self.bump_nonce_tx(&owner, tx),
            |state| {
                nonce = Some(self.apply_bump_nonce_state(state, &owner));
            },
        )?;

        Ok(nonce.expect("Nonce update should always produce a value"))
    }
}

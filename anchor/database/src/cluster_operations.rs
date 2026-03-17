use bls::PublicKeyBytes;
use rusqlite::{Transaction, params};
use ssv_types::{Cluster, ClusterId, OperatorId, Share, ValidatorMetadata};
use types::Address;

use super::{DatabaseError, NetworkDatabase, NonUniqueIndex, UniqueIndex, sql_operations};

/// Implements all cluster related functionality on the database
impl NetworkDatabase {
    pub(crate) fn insert_validator_tx(
        &self,
        cluster: &Cluster,
        validator: &ValidatorMetadata,
        shares: &[Share],
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        // Insert the top level cluster data if it does not exist, and the associated validator
        // metadata
        tx.prepare_cached(sql_operations::INSERT_CLUSTER)?
            .execute(params![
                *cluster.cluster_id,       // cluster id
                cluster.owner.to_string(), // owner
            ])?;
        tx.prepare_cached(sql_operations::INSERT_VALIDATOR)?
            .execute(params![
                validator.public_key.to_string(), // validator public key
                *cluster.cluster_id,              // cluster id
                validator.index,                  // validator index
                validator.graffiti.0.as_slice(),  // graffiti
            ])?;

        // Insert a fee recipient address if one does not already exist
        tx.execute(
            "INSERT OR IGNORE INTO owners (owner, fee_recipient) VALUES (?, ?)",
            params![cluster.owner.to_string(), cluster.owner.to_string()],
        )?;

        for share in shares {
            tx.prepare_cached(sql_operations::INSERT_CLUSTER_MEMBER)?
                .execute(params![*share.cluster_id, share.operator_id])?;
            self.insert_share(tx, share, &validator.public_key)?;
        }

        Ok(())
    }

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

    pub fn commit_validator_added(
        &self,
        cluster: Cluster,
        validator: ValidatorMetadata,
        shares: Vec<Share>,
        cursor: crate::ProcessedEventCursor,
    ) -> Result<(), DatabaseError> {
        let owner = cluster.owner;
        self.commit_db_update(
            super::ProgressUpdate::Event(cursor),
            true,
            |tx| {
                self.bump_nonce_tx(&owner, tx)?;
                self.insert_validator_tx(&cluster, &validator, &shares, tx)
            },
            |state| {
                self.apply_bump_nonce_state(state, &owner);
                self.apply_insert_validator_state(state, &cluster, &validator, &shares);
            },
        )
    }

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

    /// Inserts a new validator into the database. A new cluster will be created if this is the
    /// first validator for the cluster
    pub fn insert_validator(
        &self,
        cluster: Cluster,
        validator: &ValidatorMetadata,
        shares: Vec<Share>,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        self.insert_validator_tx(&cluster, validator, &shares, tx)?;
        self.modify_state(|state| {
            self.apply_insert_validator_state(state, &cluster, validator, &shares);
        });

        Ok(())
    }

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

    /// Mark the cluster as liquidated or active
    pub fn update_status(
        &self,
        cluster_id: ClusterId,
        status: bool,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        self.update_status_tx(cluster_id, status, tx)?;
        self.modify_state(|state| self.apply_update_status_state(state, cluster_id, status));

        Ok(())
    }

    pub(crate) fn delete_validator_tx(
        &self,
        validator_pubkey: &PublicKeyBytes,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        tx.prepare_cached(sql_operations::DELETE_VALIDATOR)?
            .execute(params![validator_pubkey.to_string()])?;

        Ok(())
    }

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

    /// Delete a validator from a cluster. This will cascade and remove all corresponding share
    /// data for this validator. If this validator is the last one in the cluster, the cluster
    /// and all corresponding cluster members will also be removed
    pub fn delete_validator(
        &self,
        validator_pubkey: &PublicKeyBytes,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        self.delete_validator_tx(validator_pubkey, tx)?;
        self.modify_state(|state| self.apply_delete_validator_state(state, validator_pubkey));

        Ok(())
    }

    pub(crate) fn bump_nonce_tx(
        &self,
        owner: &Address,
        tx: &Transaction<'_>,
    ) -> Result<(), DatabaseError> {
        tx.prepare_cached(sql_operations::BUMP_NONCE)?
            .execute(params![owner.to_string()])?;

        Ok(())
    }

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

    /// Bump the nonce of the owner
    pub fn bump_and_get_nonce(
        &self,
        owner: &Address,
        tx: &Transaction<'_>,
    ) -> Result<u16, DatabaseError> {
        self.bump_nonce_tx(owner, tx)?;

        let mut nonce = None;
        self.modify_state(|state| {
            nonce = Some(self.apply_bump_nonce_state(state, owner));
        });
        Ok(nonce.expect("Nonce update should always produce a value"))
    }
}

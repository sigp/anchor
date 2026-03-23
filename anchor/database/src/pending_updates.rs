use bls::PublicKeyBytes;
use ssv_types::{Cluster, ClusterId, Operator, OperatorId, Share, ValidatorMetadata};

use super::{Address, Graffiti, NetworkState};
use crate::multi_index::{NonUniqueIndex, UniqueIndex};

#[derive(Debug)]
struct InsertValidatorStateUpdate {
    cluster: Cluster,
    validator: ValidatorMetadata,
    own_share: Option<Share>,
}

#[derive(Debug)]
enum StateUpdate {
    SetLastProcessedBlock(u64),
    SetMaxOperatorIdSeen(u64),
    InsertOperator {
        operator: Operator,
        is_own_operator: bool,
    },
    DeleteOperator {
        operator_id: OperatorId,
    },
    InsertValidator(Box<InsertValidatorStateUpdate>),
    UpdateClusterStatus {
        cluster_id: ClusterId,
        liquidated: bool,
    },
    DeleteValidator {
        validator_pubkey: PublicKeyBytes,
    },
    SetOwnerNonce {
        owner: Address,
        nonce: u16,
    },
    UpdateFeeRecipient {
        owner: Address,
        fee_recipient: Address,
    },
    UpdateGraffiti {
        validator_pubkey: PublicKeyBytes,
        graffiti: Graffiti,
    },
}

/// Ordered in-memory state operations to replay once the caller reaches the chosen transaction
/// boundary. This remains ordered because a block can interleave different kinds of updates in a
/// single transaction, and replaying them out of order can change the resulting in-memory state.
#[derive(Default)]
pub struct PendingStateUpdates {
    updates: Vec<StateUpdate>,
}

impl PendingStateUpdates {
    pub(crate) fn set_last_processed_block(&mut self, block_number: u64) {
        self.updates
            .push(StateUpdate::SetLastProcessedBlock(block_number));
    }

    pub(crate) fn set_max_operator_id_seen(&mut self, operator_id: u64) {
        self.updates
            .push(StateUpdate::SetMaxOperatorIdSeen(operator_id));
    }

    pub(crate) fn insert_operator(&mut self, operator: Operator, is_own_operator: bool) {
        self.updates.push(StateUpdate::InsertOperator {
            operator,
            is_own_operator,
        });
    }

    pub(crate) fn delete_operator(&mut self, operator_id: OperatorId) {
        self.updates
            .push(StateUpdate::DeleteOperator { operator_id });
    }

    pub(crate) fn insert_validator(
        &mut self,
        cluster: Cluster,
        validator: ValidatorMetadata,
        own_share: Option<Share>,
    ) {
        self.updates.push(StateUpdate::InsertValidator(Box::new(
            InsertValidatorStateUpdate {
                cluster,
                validator,
                own_share,
            },
        )));
    }

    pub(crate) fn update_cluster_status(&mut self, cluster_id: ClusterId, liquidated: bool) {
        self.updates.push(StateUpdate::UpdateClusterStatus {
            cluster_id,
            liquidated,
        });
    }

    pub(crate) fn delete_validator(&mut self, validator_pubkey: PublicKeyBytes) {
        self.updates
            .push(StateUpdate::DeleteValidator { validator_pubkey });
    }

    pub(crate) fn set_owner_nonce(&mut self, owner: Address, nonce: u16) {
        self.updates
            .push(StateUpdate::SetOwnerNonce { owner, nonce });
    }

    pub(crate) fn update_fee_recipient(&mut self, owner: Address, fee_recipient: Address) {
        self.updates.push(StateUpdate::UpdateFeeRecipient {
            owner,
            fee_recipient,
        });
    }

    pub(crate) fn update_graffiti(&mut self, validator_pubkey: PublicKeyBytes, graffiti: Graffiti) {
        self.updates.push(StateUpdate::UpdateGraffiti {
            validator_pubkey,
            graffiti,
        });
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.updates.is_empty()
    }

    pub(crate) fn apply(self, state: &mut NetworkState) {
        for update in self.updates {
            update.apply(state);
        }
    }
}

impl StateUpdate {
    fn apply(self, state: &mut NetworkState) {
        match self {
            Self::SetLastProcessedBlock(block_number) => {
                state.single_state.last_processed_block = block_number;
            }
            Self::SetMaxOperatorIdSeen(operator_id) => {
                state.single_state.max_operator_id_seen = Some(operator_id);
            }
            Self::InsertOperator {
                operator,
                is_own_operator,
            } => {
                if state.single_state.id.is_none() && is_own_operator {
                    state.single_state.id = Some(operator.id);
                }

                state.single_state.operators.insert(operator.id, operator);
            }
            Self::DeleteOperator { operator_id } => {
                state.single_state.operators.remove(&operator_id);
            }
            Self::InsertValidator(update) => {
                let InsertValidatorStateUpdate {
                    cluster,
                    validator,
                    own_share,
                } = *update;
                let validator_public_key = validator.public_key;
                let cluster_id = cluster.cluster_id;
                let cluster_owner = cluster.owner;
                let committee_id = cluster.committee_id();

                if let Some(share) = own_share {
                    state.single_state.clusters.insert(cluster_id);
                    state.multi_state.shares.insert_or_update(
                        &validator_public_key,
                        &cluster_id,
                        &cluster_owner,
                        &committee_id,
                        share,
                    );
                }

                state.multi_state.clusters.insert_or_update(
                    &cluster_id,
                    &validator_public_key,
                    &cluster_owner,
                    &committee_id,
                    cluster.clone(),
                );
                state.multi_state.validator_metadata.insert_or_update(
                    &validator_public_key,
                    &cluster_id,
                    &cluster_owner,
                    &committee_id,
                    validator,
                );
            }
            Self::UpdateClusterStatus {
                cluster_id,
                liquidated,
            } => {
                if let Some(cluster) = state.multi_state.clusters.get_mut_by(&cluster_id) {
                    cluster.liquidated = liquidated;
                }
            }
            Self::DeleteValidator { validator_pubkey } => {
                state.multi_state.shares.remove(&validator_pubkey);
                // Invariant: callers only enqueue validator removal after validating the
                // validator through the current tx/database view, and replay assumes
                // NetworkState is still aligned with that state at this point. If the metadata
                // is missing here, silently continuing would hide an invariant break after the
                // share removal above and leave NetworkState partially updated.
                let metadata = state
                    .multi_state
                    .validator_metadata
                    .remove(&validator_pubkey)
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
            Self::SetOwnerNonce { owner, nonce } => {
                state.single_state.nonces.insert(owner, nonce);
            }
            Self::UpdateFeeRecipient {
                owner,
                fee_recipient,
            } => {
                state.multi_state.clusters.modify_all_by(&owner, |cluster| {
                    cluster.fee_recipient = fee_recipient;
                });
            }
            Self::UpdateGraffiti {
                validator_pubkey,
                graffiti,
            } => {
                if let Some(validator) = state
                    .multi_state
                    .validator_metadata
                    .get_mut_by(&validator_pubkey)
                {
                    validator.graffiti = graffiti;
                }
            }
        }
    }
}

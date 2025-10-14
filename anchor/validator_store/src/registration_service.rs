//! Coordinates periodic MEV/Builder **validator registrations** for Anchor.
//!
//! Why this exists: in DVT the validator key is split, and the **registration payload includes
//! a timestamp**; if different operators construct it at different moments they’ll sign different
//! bytes. Anchor therefore **owns the timing** of registration (instead of Lighthouse’s
//! `preparation_service`) so all operators sign the **same** message and submit it to relays via
//! BN.
//!
//! This module:
//! - derives a **slot-start** timestamp (deterministic) for each window,
//! - builds `ValidatorRegistrationData` from proposer prefs (fee recipient, gas limit) and pubkey,
//! - requests signatures from the `ValidatorStore`,
//! - and submits `SignedValidatorRegistrationData` to connected beacon nodes (Builder API).

use std::sync::Arc;

use beacon_node_fallback::BeaconNodeFallback;
use futures::future::join_all;
use slot_clock::SlotClock;
use task_executor::TaskExecutor;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};
use types::{
    ChainSpec, EthSpec, PublicKeyBytes, SignedValidatorRegistrationData, Slot,
    ValidatorRegistrationData,
};
use validator_store::{DoppelgangerStatus, ValidatorStore};

/// Number of epochs to wait before re-submitting validator registration.
const EPOCHS_PER_VALIDATOR_REGISTRATION_SUBMISSION: u64 = 10;

pub struct RegistrationService<S, T> {
    inner: Arc<Inner<S, T>>,
}

struct Inner<S, T> {
    validator_store: Arc<S>,
    slot_clock: T,
    beacon_nodes: Arc<BeaconNodeFallback<T>>,
    executor: TaskExecutor,
}

impl<S: ValidatorStore + 'static, T: SlotClock + 'static> RegistrationService<S, T> {
    pub fn new(
        validator_store: Arc<S>,
        slot_clock: T,
        beacon_nodes: Arc<BeaconNodeFallback<T>>,
        executor: TaskExecutor,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                validator_store,
                slot_clock,
                beacon_nodes,
                executor,
            }),
        }
    }

    /// Starts the service which periodically sends connected beacon nodes validator registration
    /// information.
    pub fn start_validator_registration_service(self, spec: &ChainSpec) -> Result<(), String> {
        info!("Validator registration service started");

        let spec = spec.clone();
        let slot_duration = Duration::from_secs(spec.seconds_per_slot);

        let executor = self.inner.executor.clone();

        let validator_registration_fut = async move {
            loop {
                if let Some(slot) = self.inner.slot_clock.now() {
                    let inner = self.inner.clone();
                    let executor = inner.executor.clone();
                    let future = async move {
                        // Poll the endpoint immediately to ensure fee recipients are received.
                        if let Err(e) = inner.register_validators(slot).await {
                            error!(error = ?e, "Error during validator registration");
                        }
                    };
                    executor.spawn(future, "validator_registration");
                } else {
                    error!("Slot clock can not return current slot");
                }

                // Wait one slot if the register validator request fails or if we should not publish
                // at the current slot.
                if let Some(duration_to_next_slot) = self.inner.slot_clock.duration_to_next_slot() {
                    sleep(duration_to_next_slot).await;
                } else {
                    error!("Failed to read slot clock");
                    // If we can't read the slot clock, just wait another slot.
                    sleep(slot_duration).await;
                }
            }
        };
        executor.spawn(validator_registration_fut, "validator_registration_service");
        Ok(())
    }
}

impl<S: ValidatorStore + 'static, T: SlotClock + 'static> Inner<S, T> {
    fn collect_validator_registration_data(
        &self,
        slot: Slot,
        number_of_slots_between_registrations: u64,
    ) -> Vec<ValidatorRegistrationData> {
        let all_pubkeys: Vec<_> = self
            .validator_store
            .voting_pubkeys(DoppelgangerStatus::ignored);

        let Some(timestamp) = self
            .slot_clock
            .start_of(slot)
            .map(|duration| duration.as_secs())
        else {
            // Try again later.
            return vec![];
        };

        all_pubkeys
            .into_iter()
            .filter_map(|pubkey| {
                self.get_registration_data_for_pubkey(
                    pubkey,
                    timestamp,
                    slot,
                    number_of_slots_between_registrations,
                )
            })
            .collect()
    }

    fn get_registration_data_for_pubkey(
        &self,
        pubkey: PublicKeyBytes,
        timestamp: u64,
        slot: Slot,
        number_of_slots_between_registrations: u64,
    ) -> Option<ValidatorRegistrationData> {
        let proposal_data = self.validator_store.proposal_data(&pubkey)?;
        // Ignore fee recipients for keys without indices, they are inactive.
        let index = proposal_data.validator_index?;

        if is_scheduled_for_slot(slot, index, number_of_slots_between_registrations) {
            return None;
        }

        // We don't log for missing fee recipients here because this will be logged more
        // frequently in `collect_preparation_data`.
        proposal_data.fee_recipient.and_then(|fee_recipient| {
            proposal_data
                .builder_proposals
                .then_some(ValidatorRegistrationData {
                    fee_recipient,
                    gas_limit: proposal_data.gas_limit,
                    pubkey,
                    timestamp,
                })
        })
    }

    async fn sign_registration_data(
        &self,
        registration_data: Vec<ValidatorRegistrationData>,
    ) -> Vec<SignedValidatorRegistrationData> {
        // Execute signing in parallel
        let results = join_all(registration_data.into_iter().map(|data| async {
            (
                data.pubkey,
                self.validator_store
                    .sign_validator_registration_data(data)
                    .await,
            )
        }))
        .await;
        results
            .into_iter()
            .filter_map(|(validator, result)| match result {
                Ok(signed) => Some(signed),
                Err(err) => {
                    warn!(?err, %validator, "Failed to sign validator MEV registration");
                    None
                }
            })
            .collect()
    }

    async fn broadcast_registration_data(&self, signed: &[SignedValidatorRegistrationData]) {
        if !signed.is_empty() {
            match self
                .beacon_nodes
                .broadcast(|beacon_node| async move {
                    beacon_node.post_validator_register_validator(signed).await
                })
                .await
            {
                Ok(()) => info!(
                    count = signed.len(),
                    "Published validator registrations to the builder network"
                ),
                Err(err) => warn!(
                    %err,
                    "Unable to publish validator registrations to the builder network"
                ),
            }
        }
    }

    /// Register validators with builders, used in the blinded block proposal flow.
    async fn register_validators(&self, slot: Slot) -> Result<(), String> {
        let number_of_slots_between_registrations =
            EPOCHS_PER_VALIDATOR_REGISTRATION_SUBMISSION * S::E::slots_per_epoch();
        let registration_data =
            self.collect_validator_registration_data(slot, number_of_slots_between_registrations);
        let signed = self.sign_registration_data(registration_data).await;
        self.broadcast_registration_data(&signed).await;
        Ok(())
    }
}

/// To not sign for all validators at once, select based on the current slot.
/// Note that it is important that this is the same across client implementations, as else
/// the nodes broadcast partial signatures for their validators at varying times.
/// Assigns each validator to one of `number_of_slots_between_registrations` slot buckets by `index`
/// % `number_of_slots_between_registrations`. In slot s, we serve bucket s %
/// `number_of_slots_between_registrations`. This ensures each validator refreshes once every
/// `number_of_slots_between_registrations` slots.
fn is_scheduled_for_slot(
    slot: Slot,
    index: u64,
    number_of_slots_between_registrations: u64,
) -> bool {
    slot % number_of_slots_between_registrations != index % number_of_slots_between_registrations
}

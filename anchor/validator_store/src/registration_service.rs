use std::sync::Arc;

use beacon_node_fallback::BeaconNodeFallback;
use futures::future::join_all;
use slot_clock::SlotClock;
use task_executor::TaskExecutor;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};
use types::{ChainSpec, EthSpec, SignedValidatorRegistrationData, Slot, ValidatorRegistrationData};
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
        slots_per_registration: u64,
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
                let proposal_data = self.validator_store.proposal_data(&pubkey)?;
                // Ignore fee recipients for keys without indices, they are inactive.
                let index = proposal_data.validator_index?;

                // To not sign for all validators at once, select based on the current slot.
                if slot % slots_per_registration != index % slots_per_registration {
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
            })
            .collect()
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
        let slots_per_registration =
            EPOCHS_PER_VALIDATOR_REGISTRATION_SUBMISSION * S::E::slots_per_epoch();
        let registration_data =
            self.collect_validator_registration_data(slot, slots_per_registration);
        let signed = self.sign_registration_data(registration_data).await;
        self.broadcast_registration_data(&signed).await;
        Ok(())
    }
}

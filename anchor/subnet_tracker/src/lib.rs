use alloy::primitives::ruint::aliases::U256;
use database::{NetworkState, UniqueIndex};
use serde::{Deserialize, Serialize};
use ssv_types::CommitteeId;
use std::collections::HashSet;
use std::ops::Deref;
use std::time::Duration;
use task_executor::TaskExecutor;
use tokio::sync::{mpsc, watch};
use tokio::time::sleep;
use tracing::{debug, error, warn};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubnetId(#[serde(with = "serde_utils::quoted_u64")] u64);

impl SubnetId {
    pub fn new(id: u64) -> Self {
        id.into()
    }

    pub fn from_committee(committee_id: CommitteeId, subnet_count: usize) -> Self {
        // Derive a numeric "committee ID" and convert to an index in [0..subnet_count].
        let id = U256::from_be_bytes(*committee_id);
        SubnetId(
            (id % U256::from(subnet_count))
                .try_into()
                .expect("modulo must be < subnet_count"),
        )
    }
}

impl From<u64> for SubnetId {
    fn from(x: u64) -> Self {
        Self(x)
    }
}

impl Deref for SubnetId {
    type Target = u64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub enum SubnetEvent {
    Join(SubnetId),
    Leave(SubnetId),
}

pub fn start_subnet_tracker(
    db: watch::Receiver<NetworkState>,
    subnet_count: usize,
    subscribe_all_subnets: bool,
    executor: &TaskExecutor,
) -> mpsc::Receiver<SubnetEvent> {
    if !subscribe_all_subnets {
        // a channel capacity of 1 is fine - the subnet_tracker does not do anything else, it can wait.
        let (tx, rx) = mpsc::channel(1);
        executor.spawn(subnet_tracker(tx, db, subnet_count), "subnet_tracker");
        rx
    } else {
        let (tx, rx) = mpsc::channel(subnet_count);
        for subnet in (0..(subnet_count as u64)).map(SubnetId) {
            if let Err(err) = tx.try_send(SubnetEvent::Join(subnet)) {
                error!(?err, "Impossible error while subscribing to all subnets");
            }
        }
        rx
    }
}

/// The main background task:
/// - Gathers the current subnets from `NetworkState`.
/// - Compares them to the previously-seen subnets.
/// - Emits `Join` events for newly-added subnets and `Leave` events for removed subnets.
async fn subnet_tracker(
    tx: mpsc::Sender<SubnetEvent>,
    mut db: watch::Receiver<NetworkState>,
    subnet_count: usize,
) {
    // `previous_subnets` tracks which subnets were joined in the last iteration.
    let mut previous_subnets = HashSet::new();

    loop {
        // Build the `current_subnets` set by examining the clusters we own.
        let mut current_subnets = HashSet::new();

        // do not await while holding lock!
        // explicit scope needed because rustc cant handle equivalent drop(state)
        {
            // Acquire the current snapshot of the database state (this is synchronous).
            let state = db.borrow();
            for cluster_id in state.get_own_clusters() {
                if let Some(cluster) = state.clusters().get_by(cluster_id) {
                    let subnet_id = SubnetId::from_committee(cluster.committee_id(), subnet_count);
                    current_subnets.insert(subnet_id);
                }
            }
        }

        // For every subnet that was previously joined but is no longer in `current_subnets`,
        // send a `Leave` event.
        for subnet in previous_subnets.difference(&current_subnets) {
            debug!(?subnet, "send leave");
            if tx.send(SubnetEvent::Leave(*subnet)).await.is_err() {
                warn!("Network no longer listening for subnets");
                return;
            }
        }

        // For every subnet that was not previously joined but is now in `current_subnets`,
        // send a `Join` event.
        for subnet in current_subnets.difference(&previous_subnets) {
            debug!(?subnet, "send join");
            if tx.send(SubnetEvent::Join(*subnet)).await.is_err() {
                warn!("Network no longer listening for subnets");
                return;
            }
        }

        // Update `previous_subnets` to reflect the current snapshot for the next iteration.
        previous_subnets = current_subnets;

        // Wait for the watch channel to signal a changed value before re-running the loop.
        if db.changed().await.is_err() {
            warn!("Database no longer provides updates");
            return;
        }
    }
}

/// only useful for testing - introduce feature flag?
pub fn test_tracker(
    executor: TaskExecutor,
    events: Vec<SubnetEvent>,
    msg_delay: Duration,
) -> mpsc::Receiver<SubnetEvent> {
    let (tx, rx) = mpsc::channel(1);

    executor.spawn(
        async move {
            for event in events {
                sleep(msg_delay).await;
                tx.send(event).await.unwrap();
            }
            while !tx.is_closed() {
                sleep(Duration::from_millis(100)).await;
            }
        },
        "test_subnet_tracker",
    );

    rx
}

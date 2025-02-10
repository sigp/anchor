use alloy::primitives::keccak256;
use alloy::primitives::ruint::aliases::U256;
use database::{NetworkState, UniqueIndex};
use log::warn;
use serde::{Deserialize, Serialize};
use ssv_types::Cluster;
use std::collections::HashSet;
use std::ops::Deref;
use std::time::Duration;
use task_executor::TaskExecutor;
use tokio::sync::{mpsc, watch};
use tokio::time::sleep;
use tracing::debug;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubnetId(#[serde(with = "serde_utils::quoted_u64")] u64);

impl SubnetId {
    pub fn new(id: u64) -> Self {
        id.into()
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
    executor: &TaskExecutor,
) -> mpsc::Receiver<SubnetEvent> {
    // a channel capacity of 1 is fine - the subnet_tracker does not do anything else, it can wait.
    let (tx, rx) = mpsc::channel(1);
    executor.spawn(subnet_tracker(tx, db, subnet_count), "subnet_tracker");
    rx
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
                    // Derive a numeric "committee ID" and convert to an index in [0..subnet_count].
                    let id = get_committee_id(&cluster);
                    let index = (id % U256::from(subnet_count))
                        .try_into()
                        .expect("modulo must be < subnet_count");
                    current_subnets.insert(index);
                }
            }
        }

        // For every subnet that was previously joined but is no longer in `current_subnets`,
        // send a `Leave` event.
        for subnet in previous_subnets.difference(&current_subnets) {
            debug!(?subnet, "send leave");
            if tx
                .send(SubnetEvent::Leave(SubnetId(*subnet)))
                .await
                .is_err()
            {
                warn!("Network no longer listening for subnets");
                return;
            }
        }

        // For every subnet that was not previously joined but is now in `current_subnets`,
        // send a `Join` event.
        for subnet in current_subnets.difference(&previous_subnets) {
            debug!(?subnet, "send join");
            if tx.send(SubnetEvent::Join(SubnetId(*subnet))).await.is_err() {
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

fn get_committee_id(cluster: &Cluster) -> U256 {
    let mut operator_ids = cluster
        .cluster_members
        .iter()
        .map(|x| **x)
        .collect::<Vec<_>>();
    // Sort the operator IDs
    operator_ids.sort();
    let mut data: Vec<u8> = Vec::with_capacity(operator_ids.len() * 4);

    // Add the operator IDs as 32 byte values
    for id in operator_ids {
        data.extend_from_slice(&id.to_le_bytes());
    }

    // Hash it all
    U256::from_be_bytes(keccak256(data).0)
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

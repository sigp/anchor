use alloy::primitives::keccak256;
use alloy::primitives::ruint::aliases::U256;
use database::{NetworkState, UniqueIndex};
use log::warn;
use serde::{Deserialize, Serialize};
use ssv_types::Cluster;
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

pub struct SubnetTracker {
    events: mpsc::Receiver<SubnetEvent>,
}

impl SubnetTracker {
    pub async fn recv(&mut self) -> Option<SubnetEvent> {
        self.events.recv().await
    }
}

pub fn start_subnet_tracker(
    db: watch::Receiver<NetworkState>,
    subnet_count: usize,
    executor: &TaskExecutor,
) -> SubnetTracker {
    // a channel capacity of 1 is fine - the subnet_tracker does not do anything else, it can wait.
    let (tx, rx) = mpsc::channel(1);
    executor.spawn(subnet_tracker(tx, db, subnet_count), "subnet_tracker");
    SubnetTracker { events: rx }
}

#[derive(Clone, Debug)]
enum JoinState {
    Not,
    Old,
    New,
    Still,
}

async fn subnet_tracker(
    tx: mpsc::Sender<SubnetEvent>,
    mut db: watch::Receiver<NetworkState>,
    subnet_count: usize,
) {
    let mut join_states = vec![JoinState::Not; subnet_count];
    loop {
        debug!("subnet tracker starting update");
        // do not await while holding lock!
        // explicit scope needed because rustc cant handle equivalent drop(state)
        {
            let state = db.borrow();
            for cluster_id in state.get_own_clusters() {
                let Some(cluster) = state.clusters().get_by(cluster_id) else {
                    continue;
                };
                let id = get_committee_id(&cluster);
                let subnet = id % U256::from(subnet_count);
                let subnet: usize = subnet
                    .try_into()
                    .expect("modulo guaranteed to produce low enough value");
                // update join state for later sending to networking
                match join_states[subnet] {
                    JoinState::Not => {
                        // mark to be subscribed
                        join_states[subnet] = JoinState::New;
                    }
                    JoinState::Old => {
                        // mark to NOT be unsubscibed subscribed
                        join_states[subnet] = JoinState::Still;
                    }
                    _ => {}
                }
            }
        }

        for (subnet, join_state) in join_states.iter_mut().enumerate() {
            let send_result = match join_state {
                // nothing to do :)
                JoinState::Not => continue,
                // this was not marked as still joined -> unsub
                JoinState::Old => {
                    *join_state = JoinState::Not;
                    debug!(?subnet, "send leave");
                    tx.send(SubnetEvent::Leave(SubnetId(subnet as u64)))
                }
                // newly joined -> sub and mark as "old joined" for next round
                JoinState::New => {
                    *join_state = JoinState::Old;
                    debug!(?subnet, "send join");
                    tx.send(SubnetEvent::Join(SubnetId(subnet as u64)))
                }
                // still joined -> nothing to do, reset for next round
                JoinState::Still => {
                    *join_state = JoinState::Old;
                    continue;
                }
            }
            .await;
            if send_result.is_err() {
                warn!("Network no longer listening for subnets");
                return;
            }
        }

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
) -> SubnetTracker {
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

    SubnetTracker { events: rx }
}

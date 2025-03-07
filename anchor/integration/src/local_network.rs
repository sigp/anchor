use node_test_rig::{LocalBeaconNode, LocalValidatorClient};
use types::EthSpec;
use std::sync::{RwLock, Arc};

pub struct SsvNetworkParams {
    pub num_operators: usize,
    pub num_validators: usize,
}

impl SsvNetworkParams {
    // Default network state based on the hardcoded db
    fn default() -> Self {
        Self {
            num_operators: 4,
            num_validators: 1
        }
    }
}

pub struct Inner<E: EthSpec> {
    pub validators: RwLock<Vec<LocalBeaconNode<E>>>,
    pub beacon_nodes: RwLock<Vec<LocalBeaconNode<E>>>,
}

pub struct SsvLocalNetwork<E: EthSpec> {
    pub params: SsvNetworkParams,
    pub inner: Arc<Inner<E>>

}

impl<E: EthSpec> SsvLocalNetwork<E> {
    pub fn create_local_network() -> SsvLocalNetwork<E> {
        Self {
            params: SsvNetworkParams::default(),
            inner: Arc::new(Inner {
                validators: RwLock::new(Vec::new()),
                beacon_nodes: RwLock::new(Vec::new())
            })
        }
    }

    pub fn add_execution_node() {
        todo!()
    }

    pub fn add_beacon_node() {
        todo!()
    }

    pub fn add_operator_node() {
        todo!()
    }

    pub fn add_validator_node() {
        todo!()

    }
}


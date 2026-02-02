//! Gossipsub topic scoring helpers.
//!
//! This module provides functions for calculating expected message rates
//! for gossipsub topic scoring. These rates help detect flooding and
//! underperformance on subnet topics.

use std::ops::Deref;

use database::NetworkState;
use ssv_types::CommitteeInfo;
use types::{ChainSpec, EthSpec};

use crate::{SUBNET_COUNT, SubnetId, message_rate};

/// Calculate the expected message rate for a specific subnet.
///
/// This rate is used by gossipsub to set per-topic scoring parameters that detect:
/// - Flooding (too many messages vs expected)
/// - Underperformance (too few messages vs expected)
///
/// # Arguments
///
/// * `subnet` - The subnet to calculate the rate for
/// * `network_state` - Current network state containing cluster information
/// * `chain_spec` - Chain specification for timing parameters
pub fn calculate_message_rate_for_subnet<E: EthSpec>(
    subnet: &SubnetId,
    network_state: impl Deref<Target = NetworkState>,
    chain_spec: &ChainSpec,
) -> f64 {
    let committees_info = get_committee_info_for_subnet(subnet, network_state);
    message_rate::calculate_message_rate_for_topic::<E>(&committees_info, chain_spec)
}

/// Get committee info for all clusters on a specific subnet.
///
/// This function retrieves clusters that map to the given subnet and converts
/// them to `CommitteeInfo` which includes both committee members and validator indices.
///
/// # Arguments
///
/// * `subnet` - The subnet to get committee info for
/// * `network_state` - Current network state containing cluster information
pub fn get_committee_info_for_subnet(
    subnet: &SubnetId,
    network_state: impl Deref<Target = NetworkState>,
) -> Vec<CommitteeInfo> {
    use database::NonUniqueIndex;

    network_state
        .clusters()
        .values()
        .filter(|cluster| {
            let cluster_subnet =
                SubnetId::from_committee_alan(cluster.committee_id(), SUBNET_COUNT);
            cluster_subnet == *subnet
        })
        .map(|cluster| {
            // Convert cluster to CommitteeInfo by getting validator indices
            let validator_indices = network_state
                .metadata()
                .get_all_by(&cluster.cluster_id)
                .flat_map(|metadata| metadata.index)
                .collect::<Vec<_>>();

            CommitteeInfo {
                committee_members: cluster.cluster_members.clone(),
                validator_indices,
            }
        })
        .collect()
}

use database::{NetworkState, NonUniqueIndex};
use slot_clock::SlotClock;
use ssv_types::OperatorId;
use tracing::{debug, warn};
use types::EthSpec;

use crate::service::SubnetService;
use crate::{SubnetId, TopicEvent, message_rate};

impl<S: SlotClock> SubnetService<S> {
    /// Emit updated message-rate estimates for gossipsub topic scoring.
    ///
    /// Gossipsub uses these rates to set per-topic scoring parameters that detect:
    /// - Flooding (too many messages vs expected)
    /// - Underperformance (too few messages vs expected)
    ///
    /// Rates are recalculated at each epoch because committee compositions and
    /// sync committee memberships can change.
    pub(crate) async fn send_scoring_rate_updates<E: EthSpec>(&self) {
        // Clone the subnets to avoid holding lock during async operations
        let subnets: Vec<SubnetId> = {
            let previous = self.previous_subnets.read();
            debug!(
                subnet_count = previous.len(),
                "Sending updated scoring rates for all topics"
            );
            previous.iter().copied().collect()
        };

        for subnet in subnets {
            let Some(topic) = self.current_topic_for_subnet(subnet) else {
                warn!(?subnet, "Failed to get topic for scoring rate update");
                continue;
            };

            let rate = {
                let state = self.db.borrow();
                let committees_info = self.get_committee_info_for_subnet(&subnet, &state);
                message_rate::calculate_message_rate_for_topic::<E>(
                    &committees_info,
                    &self.chain_spec,
                )
            };

            if self
                .tx
                .send(TopicEvent::RateUpdate {
                    topic,
                    message_rate: rate,
                })
                .await
                .is_err()
            {
                warn!("Network no longer listening for topic events");
                return;
            }
        }
    }

    /// Compute a subnet's message rate if scoring is enabled.
    pub(crate) fn subnet_message_rate<E: EthSpec>(
        &self,
        subnet: &SubnetId,
        network_state: &NetworkState,
    ) -> Option<f64> {
        if self.disable_gossipsub_topic_scoring {
            return None;
        }

        let committees_info = self.get_committee_info_for_subnet(subnet, network_state);
        Some(message_rate::calculate_message_rate_for_topic::<E>(
            &committees_info,
            &self.chain_spec,
        ))
    }

    /// Get committee info for all clusters on a specific subnet.
    ///
    /// This function retrieves clusters that map to the given subnet and converts
    /// them to `CommitteeInfo` which includes both committee members and validator indices.
    fn get_committee_info_for_subnet(
        &self,
        subnet: &SubnetId,
        network_state: &NetworkState,
    ) -> Vec<ssv_types::CommitteeInfo> {
        network_state
            .clusters()
            .values()
            .filter(|cluster| {
                let operator_ids: Vec<OperatorId> =
                    cluster.cluster_members.iter().copied().collect();
                match self
                    .subnet_for_committee_with_operators(cluster.committee_id(), &operator_ids)
                {
                    Ok(cluster_subnet) => cluster_subnet == *subnet,
                    Err(_) => false,
                }
            })
            .map(|cluster| {
                // Convert cluster to CommitteeInfo by getting validator indices
                let validator_indices = network_state
                    .metadata()
                    .get_all_by(&cluster.cluster_id)
                    .flat_map(|metadata| metadata.index)
                    .collect::<Vec<_>>();

                ssv_types::CommitteeInfo {
                    committee_members: cluster.cluster_members.clone(),
                    validator_indices,
                }
            })
            .collect()
    }
}

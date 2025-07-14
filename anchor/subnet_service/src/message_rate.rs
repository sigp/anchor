//! Message rate calculations for SSV topics based on committee configurations.
//!
//! This module calculates expected message rates for gossipsub topics based on
//! the number of validators and operators in committees, following the SSV
//! reference implementation from Go.

use ssv_types::CommitteeInfo;
use tracing::{debug, trace};
use types::{
    ChainSpec, EthSpec, Unsigned,
    consts::altair::{SYNC_COMMITTEE_SUBNET_COUNT, TARGET_AGGREGATORS_PER_SYNC_SUBCOMMITTEE},
};

// Ethereum network parameters (these could be made configurable in the future)
const ETHEREUM_VALIDATORS: f64 = 1_000_000.0;

// Fixed probability
const PROPOSAL_PROBABILITY: f64 = 1.0 / ETHEREUM_VALIDATORS;

/// Calculate estimated attestation committee size based on EthSpec
fn estimated_attestation_committee_size<E: EthSpec>() -> f64 {
    ETHEREUM_VALIDATORS / E::MaxValidatorsPerCommittee::to_u64() as f64
}

/// Calculate aggregator probability based on EthSpec
fn aggregator_probability<E: EthSpec>() -> f64 {
    16.0 / estimated_attestation_committee_size::<E>()
}

// For values that exceed this, the expected number of committee duties approaches zero
// TODO: It depends on duties per epoch, 32 duties per epoch maps to
// 560. If the value of duties per epoch changes, this value needs
// to be adjusted (need to run Monte Carlo simulation for that number).
const MAX_VALIDATORS_PER_COMMITTEE_LIST_CUT: usize = 560;
// Represents the limit of the number of committee duties in an epoch
// with only sync committee beacon duties (no attestation) taken for a very big number of
// validators. To help reasoning it, note that for a very big number of validators all slots in the
// epoch will have an attestation with high probability and, thus,
// the committee duties with only sync committee beacon duties tends to 0.
const SINGLE_SC_DUTIES_LIMIT: f64 = 0.0;

/// Expected number of messages for different duty types
#[derive(Debug, Clone, Copy)]
pub struct MessageCounts {
    /// Pre-consensus messages
    pub pre_consensus: usize,
    /// Consensus messages (proposal + prepares + commits + decided messages)
    pub consensus: usize,
    /// Post-consensus messages
    pub post_consensus: usize,
}

impl MessageCounts {
    /// Create message counts for consensus messages
    /// Formula: 1 Proposal + n Prepares + n Commits + 2 Decided (average)
    #[inline]
    fn consensus_messages(committee_size: usize) -> usize {
        1 + committee_size + committee_size + 2
    }

    /// Create message counts for partial signature messages
    #[inline]
    fn partial_signature_messages(committee_size: usize) -> usize {
        committee_size
    }

    /// Calculate message counts for duties with pre-consensus
    /// (Pre-Consensus + Consensus + Post-Consensus)
    #[inline]
    pub fn duty_with_pre_consensus(committee_size: usize) -> Self {
        Self {
            pre_consensus: Self::partial_signature_messages(committee_size),
            consensus: Self::consensus_messages(committee_size),
            post_consensus: Self::partial_signature_messages(committee_size),
        }
    }

    /// Calculate message counts for duties without pre-consensus
    /// (Consensus + Post-Consensus)
    #[inline]
    pub fn duty_without_pre_consensus(committee_size: usize) -> Self {
        Self {
            pre_consensus: 0,
            consensus: Self::consensus_messages(committee_size),
            post_consensus: Self::partial_signature_messages(committee_size),
        }
    }

    /// Get total message count
    #[inline]
    pub fn total(&self) -> usize {
        self.pre_consensus + self.consensus + self.post_consensus
    }
}

/// Expected number of committee duties per epoch due to attestations
fn expected_committee_duties_per_epoch_due_to_attestation<E: EthSpec>(
    num_validators: usize,
) -> f64 {
    if num_validators == 0 {
        return 0.0;
    }

    // If the committee has more validators than our limit, return the limit value
    if num_validators >= MAX_VALIDATORS_PER_COMMITTEE_LIST_CUT {
        return E::slots_per_epoch() as f64;
    }

    let k = num_validators as f64;
    let n = E::slots_per_epoch() as f64;

    // Probability that all validators are not assigned to slot i
    let probability_all_not_on_slot_i = ((n - 1.0) / n).powf(k);

    // Probability that at least one validator is assigned to slot i
    let probability_at_least_one_on_slot_i = 1.0 - probability_all_not_on_slot_i;

    // Expected number of duties per epoch
    n * probability_at_least_one_on_slot_i
}

/// Expected committee duties per epoch that are due to only sync committee beacon duties
fn expected_single_sc_committee_duties_per_epoch<E: EthSpec>(num_validators: usize) -> f64 {
    if num_validators == 0 {
        return 0.0;
    }

    // If the committee has more validators than our limit, return the limit value
    if num_validators >= MAX_VALIDATORS_PER_COMMITTEE_LIST_CUT {
        return SINGLE_SC_DUTIES_LIMIT;
    }

    let sync_committee_size = E::sync_committee_size() as f64;
    // Probability that a validator is not in sync committee
    let sync_committee_probability = sync_committee_probability(sync_committee_size);
    let chance_of_not_being_in_sync_committee = 1.0 - sync_committee_probability;

    // Probability that all validators are not in sync committee
    let chance_that_all_validators_are_not_in_sync_committee =
        chance_of_not_being_in_sync_committee.powf(num_validators as f64);

    // Probability that at least one validator is in sync committee
    let chance_of_at_least_one_validator_being_in_sync_committee =
        1.0 - chance_that_all_validators_are_not_in_sync_committee;

    // Expected number of slots with no attestation duty
    let expected_slots_with_no_duty = E::slots_per_epoch() as f64
        - expected_committee_duties_per_epoch_due_to_attestation::<E>(num_validators);

    // Expected number of committee duties per epoch created due to only sync committee duties
    chance_of_at_least_one_validator_being_in_sync_committee * expected_slots_with_no_duty
}

/// Calculates the message rate for a topic given its committees' configurations
///
/// This function calculates the expected message rate in messages per second
/// based on the committee configurations (number of operators and validators).
///
/// # Arguments
/// * `committees` - Slice of committee configurations
/// * `chain_spec` - Chain specification containing slot duration
///
/// # Returns
/// Expected message rate in messages per second  
pub fn calculate_message_rate_for_topic<E: EthSpec>(
    committees: &[CommitteeInfo],
    chain_spec: &ChainSpec,
) -> f64 {
    if committees.is_empty() {
        return 0.0;
    }

    let slots_per_epoch_f64 = E::slots_per_epoch() as f64;
    let slot_duration_seconds = chain_spec.seconds_per_slot as f64;
    let sync_committee_size = E::sync_committee_size() as f64;

    let mut total_msg_rate = 0.0;

    for committee in committees {
        let committee_size = committee.committee_members.len();
        let num_validators = committee.validator_indices.len();

        if committee_size == 0 || num_validators == 0 {
            continue;
        }

        let duties_without_pre_consensus =
            MessageCounts::duty_without_pre_consensus(committee_size).total() as f64;
        let duties_with_pre_consensus =
            MessageCounts::duty_with_pre_consensus(committee_size).total() as f64;

        // Calculate different types of duties and their message rates

        // Attestation duties (without pre-consensus)
        let attestation_duties =
            expected_committee_duties_per_epoch_due_to_attestation::<E>(num_validators)
                * duties_without_pre_consensus;

        // Sync committee duties (without pre-consensus)
        let sync_committee_duties =
            expected_single_sc_committee_duties_per_epoch::<E>(num_validators)
                * duties_without_pre_consensus;

        // Calculate sync committee probabilities dynamically
        let sync_committee_probability = sync_committee_probability(sync_committee_size);
        let sync_committee_agg_prob =
            sync_committee_agg_prob(sync_committee_size, sync_committee_probability);

        // Aggregator duties (with pre-consensus)
        let aggregator_duties =
            num_validators as f64 * aggregator_probability::<E>() * duties_with_pre_consensus;

        // Proposal duties (with pre-consensus)
        let proposal_duties = num_validators as f64
            * slots_per_epoch_f64
            * PROPOSAL_PROBABILITY
            * duties_with_pre_consensus;

        // Sync committee aggregation duties (with pre-consensus)
        let sync_agg_duties = num_validators as f64
            * slots_per_epoch_f64
            * sync_committee_agg_prob
            * duties_with_pre_consensus;

        trace!(
            committee_size,
            num_validators,
            attestation_duties,
            sync_committee_duties,
            aggregator_duties,
            proposal_duties,
            sync_agg_duties,
            "Calculated duties for committee"
        );

        total_msg_rate += attestation_duties
            + sync_committee_duties
            + aggregator_duties
            + proposal_duties
            + sync_agg_duties;
    }

    // Convert rate from messages per epoch to messages per second
    let total_epoch_seconds = slots_per_epoch_f64 * slot_duration_seconds;
    let messages_per_second = total_msg_rate / total_epoch_seconds;

    debug!(
        committees_count = committees.len(),
        total_msg_rate_per_epoch = total_msg_rate,
        total_epoch_seconds,
        messages_per_second,
        "Calculated total message rate for topic"
    );

    messages_per_second
}

fn sync_committee_probability(sync_committee_size: f64) -> f64 {
    sync_committee_size / ETHEREUM_VALIDATORS
}

fn sync_committee_agg_prob(sync_committee_size: f64, sync_committee_probability: f64) -> f64 {
    sync_committee_probability * TARGET_AGGREGATORS_PER_SYNC_SUBCOMMITTEE as f64
        / (sync_committee_size / SYNC_COMMITTEE_SUBNET_COUNT as f64)
}

#[cfg(test)]
mod tests {
    use ssv_types::{IndexSet, OperatorId, ValidatorIndex};
    use types::{ChainSpec, MainnetEthSpec};

    use super::*;

    // Use MainnetEthSpec as the test EthSpec type
    type TestEthSpec = MainnetEthSpec;

    fn create_test_committee_info(committee_size: usize, num_validators: usize) -> CommitteeInfo {
        let mut committee_members = IndexSet::new();
        for i in 0..committee_size {
            committee_members.insert(OperatorId(i as u64 + 1));
        }

        let validator_indices = (0..num_validators).map(ValidatorIndex).collect();

        CommitteeInfo {
            committee_members,
            validator_indices,
        }
    }

    #[test]
    fn test_message_counts_consensus_messages() {
        let committee_size = 4;
        let expected_consensus = 1 + committee_size + committee_size + 2; // 1 proposal + n prepares + n commits + 2 decided
        assert_eq!(
            MessageCounts::consensus_messages(committee_size),
            expected_consensus
        );
    }

    #[test]
    fn test_message_counts_partial_signature_messages() {
        let committee_size = 7;
        assert_eq!(
            MessageCounts::partial_signature_messages(committee_size),
            committee_size
        );
    }

    #[test]
    fn test_message_counts_duty_with_pre_consensus() {
        let committee_size = 4;

        let with_pre = MessageCounts::duty_with_pre_consensus(committee_size);
        assert_eq!(with_pre.pre_consensus, committee_size);
        assert_eq!(with_pre.consensus, 1 + committee_size + committee_size + 2); // 11
        assert_eq!(with_pre.post_consensus, committee_size);
        assert_eq!(with_pre.total(), 19);
    }

    #[test]
    fn test_message_counts_duty_without_pre_consensus() {
        let committee_size = 4;

        let without_pre = MessageCounts::duty_without_pre_consensus(committee_size);
        assert_eq!(without_pre.pre_consensus, 0);
        assert_eq!(without_pre.consensus, 11);
        assert_eq!(without_pre.post_consensus, 4);
        assert_eq!(without_pre.total(), 15);
    }

    #[test]
    fn test_message_counts_edge_cases() {
        // Test with zero committee size
        let zero_with_pre = MessageCounts::duty_with_pre_consensus(0);
        assert_eq!(zero_with_pre.pre_consensus, 0);
        assert_eq!(zero_with_pre.consensus, 3); // 1 + 0 + 0 + 2
        assert_eq!(zero_with_pre.post_consensus, 0);

        // Test with single member committee
        let single_with_pre = MessageCounts::duty_with_pre_consensus(1);
        assert_eq!(single_with_pre.pre_consensus, 1);
        assert_eq!(single_with_pre.consensus, 5); // 1 + 1 + 1 + 2
        assert_eq!(single_with_pre.post_consensus, 1);

        // Test with large committee
        let large_committee_size = 13;
        let large_with_pre = MessageCounts::duty_with_pre_consensus(large_committee_size);
        assert_eq!(large_with_pre.pre_consensus, large_committee_size);
        assert_eq!(
            large_with_pre.consensus,
            1 + large_committee_size + large_committee_size + 2
        );
        assert_eq!(large_with_pre.post_consensus, large_committee_size);
    }

    #[test]
    fn test_expected_committee_duties_per_epoch_due_to_attestation_zero_validators() {
        let duties = expected_committee_duties_per_epoch_due_to_attestation::<TestEthSpec>(0);
        assert_eq!(duties, 0.0);
    }

    #[test]
    fn test_expected_committee_duties_per_epoch_due_to_attestation_small_committees() {
        let test_slots_per_epoch_f64 = TestEthSpec::slots_per_epoch() as f64;

        // Test with small number of validators
        let duties_1 = expected_committee_duties_per_epoch_due_to_attestation::<TestEthSpec>(1);
        let duties_5 = expected_committee_duties_per_epoch_due_to_attestation::<TestEthSpec>(5);
        let duties_10 = expected_committee_duties_per_epoch_due_to_attestation::<TestEthSpec>(10);

        // All should be positive and finite
        assert!(duties_1 > 0.0 && duties_1.is_finite());
        assert!(duties_5 > 0.0 && duties_5.is_finite());
        assert!(duties_10 > 0.0 && duties_10.is_finite());

        // Should generally increase with more validators (probability of having duties)
        assert!(duties_5 >= duties_1);
        assert!(duties_10 >= duties_5);

        // Should be bounded by TEST_SLOTS_PER_EPOCH
        assert!(duties_1 <= test_slots_per_epoch_f64);
        assert!(duties_5 <= test_slots_per_epoch_f64);
        assert!(duties_10 <= test_slots_per_epoch_f64);
    }

    #[test]
    fn test_expected_committee_duties_per_epoch_due_to_attestation_large_committees() {
        // Test boundary condition
        let duties_max = expected_committee_duties_per_epoch_due_to_attestation::<TestEthSpec>(
            MAX_VALIDATORS_PER_COMMITTEE_LIST_CUT,
        );
        let duties_over_max = expected_committee_duties_per_epoch_due_to_attestation::<TestEthSpec>(
            MAX_VALIDATORS_PER_COMMITTEE_LIST_CUT + 100,
        );

        assert!(duties_max > 0.0 && duties_max.is_finite());
        assert_eq!(duties_over_max, TestEthSpec::slots_per_epoch() as f64);
    }

    #[test]
    fn test_expected_single_sc_committee_duties_per_epoch_zero_validators() {
        let duties = expected_single_sc_committee_duties_per_epoch::<TestEthSpec>(0);
        assert_eq!(duties, 0.0);
    }

    #[test]
    fn test_expected_single_sc_committee_duties_per_epoch_small_committees() {
        let duties_1 = expected_single_sc_committee_duties_per_epoch::<TestEthSpec>(1);
        let duties_10 = expected_single_sc_committee_duties_per_epoch::<TestEthSpec>(10);
        let duties_100 = expected_single_sc_committee_duties_per_epoch::<TestEthSpec>(100);

        // All should be non-negative and finite
        assert!(duties_1 >= 0.0 && duties_1.is_finite());
        assert!(duties_10 >= 0.0 && duties_10.is_finite());
        assert!(duties_100 >= 0.0 && duties_100.is_finite());
    }

    #[test]
    fn test_expected_single_sc_committee_duties_per_epoch_large_committees() {
        let duties_max = expected_single_sc_committee_duties_per_epoch::<TestEthSpec>(
            MAX_VALIDATORS_PER_COMMITTEE_LIST_CUT,
        );
        let duties_over_max = expected_single_sc_committee_duties_per_epoch::<TestEthSpec>(
            MAX_VALIDATORS_PER_COMMITTEE_LIST_CUT + 100,
        );

        assert!(duties_max >= 0.0 && duties_max.is_finite());
        assert_eq!(duties_over_max, SINGLE_SC_DUTIES_LIMIT);
    }

    #[test]
    fn test_calculate_message_rate_for_topic_empty() {
        let chain_spec = ChainSpec::mainnet();
        let rate = calculate_message_rate_for_topic::<TestEthSpec>(&[], &chain_spec);
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn test_calculate_message_rate_for_topic_zero_validators_committee() {
        let chain_spec = ChainSpec::mainnet();
        let committee = create_test_committee_info(4, 0);
        let rate = calculate_message_rate_for_topic::<TestEthSpec>(&[committee], &chain_spec);
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn test_calculate_message_rate_for_topic_zero_operators_committee() {
        let chain_spec = ChainSpec::mainnet();
        let committee = create_test_committee_info(0, 2);
        let rate = calculate_message_rate_for_topic::<TestEthSpec>(&[committee], &chain_spec);
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn test_calculate_message_rate_for_topic_single_committee() {
        let chain_spec = ChainSpec::mainnet();
        let committee = create_test_committee_info(4, 2);
        let rate = calculate_message_rate_for_topic::<TestEthSpec>(&[committee], &chain_spec);

        // Rate should be positive for a valid committee
        assert!(rate > 0.0);
        // Rate should be finite and reasonable
        assert!(rate.is_finite());
        assert!(rate < 1000.0); // Sanity check for reasonable upper bound
    }

    #[test]
    fn test_calculate_message_rate_for_topic_multiple_committees() {
        let chain_spec = ChainSpec::mainnet();
        let committees = vec![
            create_test_committee_info(4, 2),
            create_test_committee_info(7, 3),
        ];

        let total_rate = calculate_message_rate_for_topic::<TestEthSpec>(&committees, &chain_spec);
        let first_rate =
            calculate_message_rate_for_topic::<TestEthSpec>(&[committees[0].clone()], &chain_spec);
        let second_rate =
            calculate_message_rate_for_topic::<TestEthSpec>(&[committees[1].clone()], &chain_spec);

        // All rates should be finite
        assert!(total_rate.is_finite());
        assert!(first_rate.is_finite());
        assert!(second_rate.is_finite());

        // Total rate should be sum of individual rates (additivity property)
        assert!((total_rate - (first_rate + second_rate)).abs() < 1e-10);
    }

    #[test]
    fn test_calculate_message_rate_for_topic_large_committee() {
        // Test with committee sizes that exceed limits
        let chain_spec = ChainSpec::mainnet();
        let large_committee =
            create_test_committee_info(4, MAX_VALIDATORS_PER_COMMITTEE_LIST_CUT + 100);
        let rate = calculate_message_rate_for_topic::<TestEthSpec>(&[large_committee], &chain_spec);

        // Should handle gracefully
        assert!(rate >= 0.0);
        assert!(rate.is_finite());
    }

    #[test]
    fn test_calculate_message_rate_for_topic_scaling() {
        // Test how message rate scales with committee size and validator count
        let chain_spec = ChainSpec::mainnet();
        let small_committee = create_test_committee_info(4, 1);
        let medium_committee = create_test_committee_info(7, 5);
        let large_committee = create_test_committee_info(13, 10);

        let small_rate =
            calculate_message_rate_for_topic::<TestEthSpec>(&[small_committee], &chain_spec);
        let medium_rate =
            calculate_message_rate_for_topic::<TestEthSpec>(&[medium_committee], &chain_spec);
        let large_rate =
            calculate_message_rate_for_topic::<TestEthSpec>(&[large_committee], &chain_spec);

        // All should be positive and finite
        assert!(small_rate > 0.0 && small_rate.is_finite());
        assert!(medium_rate > 0.0 && medium_rate.is_finite());
        assert!(large_rate > 0.0 && large_rate.is_finite());

        // Generally, larger committees should have higher rates
        assert!(medium_rate >= small_rate);
        assert!(large_rate >= medium_rate);
    }

    #[test]
    fn test_duty_calculation_mathematical_properties() {
        // Test mathematical properties of duty calculations

        // For small committees, duties should be monotonically increasing
        let mut prev_duties =
            expected_committee_duties_per_epoch_due_to_attestation::<TestEthSpec>(1);
        for i in 2..=20 {
            let current_duties =
                expected_committee_duties_per_epoch_due_to_attestation::<TestEthSpec>(i);
            assert!(
                current_duties >= prev_duties,
                "Duties should be non-decreasing for small committees: {prev_duties} -> {current_duties}",
            );
            prev_duties = current_duties;
        }

        // For very large committees, should approach the limit
        let large_duties =
            expected_committee_duties_per_epoch_due_to_attestation::<TestEthSpec>(1000);
        let very_large_duties =
            expected_committee_duties_per_epoch_due_to_attestation::<TestEthSpec>(10000);
        assert!(
            (large_duties - very_large_duties).abs() < 0.1,
            "Very large committees should converge to similar duty counts"
        );
    }

    #[test]
    fn test_message_rate_for_different_committee_configurations() {
        // Test various realistic committee configurations
        let chain_spec = ChainSpec::mainnet();
        let configs = vec![
            (4, 1),    // Minimum viable committee
            (4, 10),   // Small committee
            (7, 50),   // Medium committee
            (10, 100), // Large committee
            (13, 200), // Very large committee
        ];

        for (committee_size, num_validators) in configs {
            let committee = create_test_committee_info(committee_size, num_validators);
            let rate = calculate_message_rate_for_topic::<TestEthSpec>(&[committee], &chain_spec);

            assert!(
                rate > 0.0,
                "Rate should be positive for committee_size={committee_size}, num_validators={num_validators}",
            );
            assert!(
                rate.is_finite(),
                "Rate should be finite for committee_size={committee_size}, num_validators={num_validators}",
            );
            assert!(
                rate < 10000.0,
                "Rate should be reasonable for committee_size={committee_size}, num_validators={num_validators}",
            );
        }
    }

    #[test]
    fn test_edge_case_single_validator_single_operator() {
        let chain_spec = ChainSpec::mainnet();
        let committee = create_test_committee_info(1, 1);
        let rate = calculate_message_rate_for_topic::<TestEthSpec>(&[committee], &chain_spec);

        assert!(rate > 0.0);
        assert!(rate.is_finite());
    }

    #[test]
    fn test_message_counts_total_consistency() {
        // Verify that total() method is consistent with individual counts
        for committee_size in [0, 1, 4, 7, 13] {
            let with_pre = MessageCounts::duty_with_pre_consensus(committee_size);
            let without_pre = MessageCounts::duty_without_pre_consensus(committee_size);

            assert_eq!(
                with_pre.total(),
                with_pre.pre_consensus + with_pre.consensus + with_pre.post_consensus
            );
            assert_eq!(
                without_pre.total(),
                without_pre.pre_consensus + without_pre.consensus + without_pre.post_consensus
            );

            // With pre-consensus should always have more or equal messages
            assert!(with_pre.total() >= without_pre.total());
        }
    }

    #[test]
    fn test_rate_calculation_components_isolation() {
        // Test that individual duty calculations work correctly in isolation
        let test_slots_per_epoch_f64 = TestEthSpec::slots_per_epoch() as f64;

        let committee_size = 4;
        let num_validators = 10;

        let attestation_duties =
            expected_committee_duties_per_epoch_due_to_attestation::<TestEthSpec>(num_validators);
        let sync_duties =
            expected_single_sc_committee_duties_per_epoch::<TestEthSpec>(num_validators);

        assert!(attestation_duties.is_finite());
        assert!(sync_duties.is_finite());
        assert!(attestation_duties >= 0.0);
        assert!(sync_duties >= 0.0);

        // Test message counts
        let with_pre = MessageCounts::duty_with_pre_consensus(committee_size);
        let without_pre = MessageCounts::duty_without_pre_consensus(committee_size);

        assert!(with_pre.total() > 0);
        assert!(without_pre.total() > 0);
        assert!(with_pre.total() > without_pre.total());

        // Test probability-based duty calculations
        let sync_committee_size = TestEthSpec::sync_committee_size() as f64;
        let sync_committee_probability = sync_committee_probability(sync_committee_size);
        let sync_committee_agg_prob =
            sync_committee_agg_prob(sync_committee_size, sync_committee_probability);

        let aggregator_duties = num_validators as f64 * aggregator_probability::<TestEthSpec>();
        let proposal_duties =
            num_validators as f64 * test_slots_per_epoch_f64 * PROPOSAL_PROBABILITY;
        let sync_agg_duties =
            num_validators as f64 * test_slots_per_epoch_f64 * sync_committee_agg_prob;

        assert!(aggregator_duties >= 0.0 && aggregator_duties.is_finite());
        assert!(proposal_duties >= 0.0 && proposal_duties.is_finite());
        assert!(sync_agg_duties >= 0.0 && sync_agg_duties.is_finite());
    }
}

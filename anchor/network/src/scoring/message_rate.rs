//! Message rate calculations for SSV topics based on committee configurations.
//!
//! This module calculates expected message rates for gossipsub topics based on
//! the number of validators and operators in committees, following the SSV
//! reference implementation from Go.

use std::sync::OnceLock;

use ssv_types::CommitteeInfo;
use tracing::debug;

// Ethereum network parameters (these could be made configurable in the future)
const ETHEREUM_VALIDATORS: f64 = 1_000_000.0;
const SYNC_COMMITTEE_SIZE: f64 = 512.0;
const SLOTS_PER_EPOCH: f64 = 32.0;
const SLOT_DURATION_SECONDS: f64 = 12.0;

// Derived probabilities
const ESTIMATED_ATTESTATION_COMMITTEE_SIZE: f64 = ETHEREUM_VALIDATORS / 2048.0;
const AGGREGATOR_PROBABILITY: f64 = 16.0 / ESTIMATED_ATTESTATION_COMMITTEE_SIZE;
const PROPOSAL_PROBABILITY: f64 = 1.0 / ETHEREUM_VALIDATORS;
const SYNC_COMMITTEE_PROBABILITY: f64 = SYNC_COMMITTEE_SIZE / ETHEREUM_VALIDATORS;
const SYNC_COMMITTEE_AGG_PROB: f64 =
    SYNC_COMMITTEE_PROBABILITY * 16.0 / (SYNC_COMMITTEE_SIZE / 4.0);

// Committee size limits
const MAX_VALIDATORS_PER_COMMITTEE: usize = 560;
const MAX_ATTESTATION_DUTIES_PER_EPOCH_FOR_COMMITTEE: f64 = SLOTS_PER_EPOCH;
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
    fn consensus_messages(committee_size: usize) -> usize {
        1 + committee_size + committee_size + 2
    }

    /// Create message counts for partial signature messages
    fn partial_signature_messages(committee_size: usize) -> usize {
        committee_size
    }

    /// Calculate message counts for duties with pre-consensus
    /// (Pre-Consensus + Consensus + Post-Consensus)
    pub fn duty_with_pre_consensus(committee_size: usize) -> Self {
        Self {
            pre_consensus: Self::partial_signature_messages(committee_size),
            consensus: Self::consensus_messages(committee_size),
            post_consensus: Self::partial_signature_messages(committee_size),
        }
    }

    /// Calculate message counts for duties without pre-consensus
    /// (Consensus + Post-Consensus)
    pub fn duty_without_pre_consensus(committee_size: usize) -> Self {
        Self {
            pre_consensus: 0,
            consensus: Self::consensus_messages(committee_size),
            post_consensus: Self::partial_signature_messages(committee_size),
        }
    }

    /// Get total message count
    pub fn total(&self) -> usize {
        self.pre_consensus + self.consensus + self.post_consensus
    }
}

/// Expected number of committee duties per epoch due to attestations
fn expected_committee_duties_per_epoch_due_to_attestation(num_validators: usize) -> f64 {
    if num_validators == 0 {
        return 0.0;
    }

    // If the committee has more validators than our limit, return the limit value
    if num_validators >= MAX_VALIDATORS_PER_COMMITTEE {
        return MAX_ATTESTATION_DUTIES_PER_EPOCH_FOR_COMMITTEE;
    }

    let k = num_validators as f64;
    let n = SLOTS_PER_EPOCH;

    // Probability that all validators are not assigned to slot i
    let probability_all_not_on_slot_i = ((n - 1.0) / n).powf(k);

    // Probability that at least one validator is assigned to slot i
    let probability_at_least_one_on_slot_i = 1.0 - probability_all_not_on_slot_i;

    // Expected number of duties per epoch
    n * probability_at_least_one_on_slot_i
}

/// Expected committee duties per epoch that are due to only sync committee beacon duties
fn expected_single_sc_committee_duties_per_epoch(num_validators: usize) -> f64 {
    if num_validators == 0 {
        return 0.0;
    }

    // If the committee has more validators than our limit, return the limit value
    if num_validators >= MAX_VALIDATORS_PER_COMMITTEE {
        return SINGLE_SC_DUTIES_LIMIT;
    }

    // Probability that a validator is not in sync committee
    let chance_of_not_being_in_sync_committee = 1.0 - SYNC_COMMITTEE_PROBABILITY;

    // Probability that all validators are not in sync committee
    let chance_that_all_validators_are_not_in_sync_committee =
        chance_of_not_being_in_sync_committee.powf(num_validators as f64);

    // Probability that at least one validator is in sync committee
    let chance_of_at_least_one_validator_being_in_sync_committee =
        1.0 - chance_that_all_validators_are_not_in_sync_committee;

    // Expected number of slots with no attestation duty
    let expected_slots_with_no_duty = SLOTS_PER_EPOCH
        - expected_committee_duties_per_epoch_due_to_attestation_cached(num_validators);

    // Expected number of committee duties per epoch created due to only sync committee duties
    chance_of_at_least_one_validator_being_in_sync_committee * expected_slots_with_no_duty
}

/// Generates cached values for a given function up to a threshold
fn generate_cached_values<F>(generator: F, threshold: usize) -> Vec<f64>
where
    F: Fn(usize) -> f64,
{
    (0..threshold).map(generator).collect()
}

/// Pre-computed cache for attestation duties
static GENERATED_EXPECTED_COMMITTEE_DUTIES_DUE_TO_ATTESTATION: OnceLock<Vec<f64>> = OnceLock::new();

/// Pre-computed cache for sync committee duties
static GENERATED_EXPECTED_SINGLE_SC_COMMITTEE_DUTIES: OnceLock<Vec<f64>> = OnceLock::new();

/// Cached version of expected_committee_duties_per_epoch_due_to_attestation
pub fn expected_committee_duties_per_epoch_due_to_attestation_cached(num_validators: usize) -> f64 {
    // If the committee has more validators than our computed cache, return the limit value
    if num_validators >= MAX_VALIDATORS_PER_COMMITTEE {
        return MAX_ATTESTATION_DUTIES_PER_EPOCH_FOR_COMMITTEE;
    }

    let cache = GENERATED_EXPECTED_COMMITTEE_DUTIES_DUE_TO_ATTESTATION.get_or_init(|| {
        generate_cached_values(
            expected_committee_duties_per_epoch_due_to_attestation,
            MAX_VALIDATORS_PER_COMMITTEE,
        )
    });

    cache[num_validators]
}

/// Cached version of expected_single_sc_committee_duties_per_epoch
pub fn expected_single_sc_committee_duties_per_epoch_cached(num_validators: usize) -> f64 {
    // If the committee has more validators than our computed cache, return the limit value
    if num_validators >= MAX_VALIDATORS_PER_COMMITTEE {
        return SINGLE_SC_DUTIES_LIMIT;
    }

    let cache = GENERATED_EXPECTED_SINGLE_SC_COMMITTEE_DUTIES.get_or_init(|| {
        generate_cached_values(
            expected_single_sc_committee_duties_per_epoch,
            MAX_VALIDATORS_PER_COMMITTEE,
        )
    });

    cache[num_validators]
}

/// Calculates the message rate for a topic given its committees' configurations
///
/// This function calculates the expected message rate in messages per second
/// based on the committee configurations (number of operators and validators).
///
/// # Arguments
/// * `committees` - Slice of committee configurations
///
/// # Returns
/// Expected message rate in messages per second
pub fn calculate_message_rate_for_topic(committees: &[CommitteeInfo]) -> f64 {
    if committees.is_empty() {
        return 0.0;
    }

    let mut total_msg_rate = 0.0;

    for committee in committees {
        let committee_size = committee.committee_members.len();
        let num_validators = committee.validator_indices.len();

        let duties_without_pre_consensus =
            MessageCounts::duty_without_pre_consensus(committee_size).total() as f64;
        let duties_with_pre_consensus =
            MessageCounts::duty_with_pre_consensus(committee_size).total() as f64;

        if committee_size == 0 || num_validators == 0 {
            continue;
        }

        // Calculate different types of duties and their message rates

        // Attestation duties (without pre-consensus)
        let attestation_duties =
            expected_committee_duties_per_epoch_due_to_attestation_cached(num_validators);
        let attestation_msg_count = duties_without_pre_consensus;
        total_msg_rate += attestation_duties * attestation_msg_count;

        // Sync committee duties (without pre-consensus)
        let sync_committee_duties =
            expected_single_sc_committee_duties_per_epoch_cached(num_validators);
        let sync_committee_msg_count = duties_without_pre_consensus;
        total_msg_rate += sync_committee_duties * sync_committee_msg_count;

        // Aggregator duties (with pre-consensus)
        let aggregator_duties = num_validators as f64 * AGGREGATOR_PROBABILITY;
        let aggregator_msg_count = duties_with_pre_consensus;
        total_msg_rate += aggregator_duties * aggregator_msg_count;

        // Proposal duties (with pre-consensus)
        let proposal_duties = num_validators as f64 * SLOTS_PER_EPOCH * PROPOSAL_PROBABILITY;
        let proposal_msg_count = duties_with_pre_consensus;
        total_msg_rate += proposal_duties * proposal_msg_count;

        // Sync committee aggregation duties (with pre-consensus)
        let sync_agg_duties = num_validators as f64 * SLOTS_PER_EPOCH * SYNC_COMMITTEE_AGG_PROB;
        let sync_agg_msg_count = duties_with_pre_consensus;
        total_msg_rate += sync_agg_duties * sync_agg_msg_count;

        debug!(
            committee_size = committee_size,
            num_validators = num_validators,
            attestation_duties = attestation_duties,
            sync_committee_duties = sync_committee_duties,
            aggregator_duties = aggregator_duties,
            proposal_duties = proposal_duties,
            sync_agg_duties = sync_agg_duties,
            "Calculated duties for committee"
        );
    }

    // Convert rate from messages per epoch to messages per second
    let total_epoch_seconds = SLOTS_PER_EPOCH * SLOT_DURATION_SECONDS;
    let messages_per_second = total_msg_rate / total_epoch_seconds;

    debug!(
        committees_count = committees.len(),
        total_msg_rate_per_epoch = total_msg_rate,
        total_epoch_seconds = total_epoch_seconds,
        messages_per_second = messages_per_second,
        "Calculated total message rate for topic"
    );

    messages_per_second
}

#[cfg(test)]
mod tests {
    use ssv_types::{IndexSet, OperatorId, ValidatorIndex};

    use super::*;

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
        let duties = expected_committee_duties_per_epoch_due_to_attestation(0);
        assert_eq!(duties, 0.0);
    }

    #[test]
    fn test_expected_committee_duties_per_epoch_due_to_attestation_small_committees() {
        // Test with small number of validators
        let duties_1 = expected_committee_duties_per_epoch_due_to_attestation(1);
        let duties_5 = expected_committee_duties_per_epoch_due_to_attestation(5);
        let duties_10 = expected_committee_duties_per_epoch_due_to_attestation(10);

        // All should be positive and finite
        assert!(duties_1 > 0.0 && duties_1.is_finite());
        assert!(duties_5 > 0.0 && duties_5.is_finite());
        assert!(duties_10 > 0.0 && duties_10.is_finite());

        // Should generally increase with more validators (probability of having duties)
        assert!(duties_5 >= duties_1);
        assert!(duties_10 >= duties_5);

        // Should be bounded by SLOTS_PER_EPOCH
        assert!(duties_1 <= SLOTS_PER_EPOCH);
        assert!(duties_5 <= SLOTS_PER_EPOCH);
        assert!(duties_10 <= SLOTS_PER_EPOCH);
    }

    #[test]
    fn test_expected_committee_duties_per_epoch_due_to_attestation_large_committees() {
        // Test boundary condition
        let duties_max =
            expected_committee_duties_per_epoch_due_to_attestation(MAX_VALIDATORS_PER_COMMITTEE);
        let duties_over_max = expected_committee_duties_per_epoch_due_to_attestation(
            MAX_VALIDATORS_PER_COMMITTEE + 100,
        );

        assert!(duties_max > 0.0 && duties_max.is_finite());
        assert_eq!(
            duties_over_max,
            MAX_ATTESTATION_DUTIES_PER_EPOCH_FOR_COMMITTEE
        );
    }

    #[test]
    fn test_expected_single_sc_committee_duties_per_epoch_zero_validators() {
        let duties = expected_single_sc_committee_duties_per_epoch(0);
        assert_eq!(duties, 0.0);
    }

    #[test]
    fn test_expected_single_sc_committee_duties_per_epoch_small_committees() {
        let duties_1 = expected_single_sc_committee_duties_per_epoch(1);
        let duties_10 = expected_single_sc_committee_duties_per_epoch(10);
        let duties_100 = expected_single_sc_committee_duties_per_epoch(100);

        // All should be non-negative and finite
        assert!(duties_1 >= 0.0 && duties_1.is_finite());
        assert!(duties_10 >= 0.0 && duties_10.is_finite());
        assert!(duties_100 >= 0.0 && duties_100.is_finite());
    }

    #[test]
    fn test_expected_single_sc_committee_duties_per_epoch_large_committees() {
        let duties_max =
            expected_single_sc_committee_duties_per_epoch(MAX_VALIDATORS_PER_COMMITTEE);
        let duties_over_max =
            expected_single_sc_committee_duties_per_epoch(MAX_VALIDATORS_PER_COMMITTEE + 100);

        assert!(duties_max >= 0.0 && duties_max.is_finite());
        assert_eq!(duties_over_max, SINGLE_SC_DUTIES_LIMIT);
    }

    #[test]
    fn test_calculate_message_rate_for_topic_empty() {
        let rate = calculate_message_rate_for_topic(&[]);
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn test_calculate_message_rate_for_topic_zero_validators_committee() {
        let committee = create_test_committee_info(4, 0);
        let rate = calculate_message_rate_for_topic(&[committee]);
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn test_calculate_message_rate_for_topic_zero_operators_committee() {
        let committee = create_test_committee_info(0, 2);
        let rate = calculate_message_rate_for_topic(&[committee]);
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn test_calculate_message_rate_for_topic_single_committee() {
        let committee = create_test_committee_info(4, 2);
        let rate = calculate_message_rate_for_topic(&[committee]);

        // Rate should be positive for a valid committee
        assert!(rate > 0.0);
        // Rate should be finite and reasonable
        assert!(rate.is_finite());
        assert!(rate < 1000.0); // Sanity check for reasonable upper bound
    }

    #[test]
    fn test_calculate_message_rate_for_topic_multiple_committees() {
        let committees = vec![
            create_test_committee_info(4, 2),
            create_test_committee_info(7, 3),
        ];

        let total_rate = calculate_message_rate_for_topic(&committees);
        let first_rate = calculate_message_rate_for_topic(&[committees[0].clone()]);
        let second_rate = calculate_message_rate_for_topic(&[committees[1].clone()]);

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
        let large_committee = create_test_committee_info(4, MAX_VALIDATORS_PER_COMMITTEE + 100);
        let rate = calculate_message_rate_for_topic(&[large_committee]);

        // Should handle gracefully
        assert!(rate >= 0.0);
        assert!(rate.is_finite());
    }

    #[test]
    fn test_calculate_message_rate_for_topic_scaling() {
        // Test how message rate scales with committee size and validator count
        let small_committee = create_test_committee_info(4, 1);
        let medium_committee = create_test_committee_info(7, 5);
        let large_committee = create_test_committee_info(13, 10);

        let small_rate = calculate_message_rate_for_topic(&[small_committee]);
        let medium_rate = calculate_message_rate_for_topic(&[medium_committee]);
        let large_rate = calculate_message_rate_for_topic(&[large_committee]);

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
        let mut prev_duties = expected_committee_duties_per_epoch_due_to_attestation(1);
        for i in 2..=20 {
            let current_duties = expected_committee_duties_per_epoch_due_to_attestation(i);
            assert!(
                current_duties >= prev_duties,
                "Duties should be non-decreasing for small committees: {prev_duties} -> {current_duties}",
            );
            prev_duties = current_duties;
        }

        // For very large committees, should approach the limit
        let large_duties = expected_committee_duties_per_epoch_due_to_attestation(1000);
        let very_large_duties = expected_committee_duties_per_epoch_due_to_attestation(10000);
        assert!(
            (large_duties - very_large_duties).abs() < 0.1,
            "Very large committees should converge to similar duty counts"
        );
    }

    #[test]
    fn test_message_rate_for_different_committee_configurations() {
        // Test various realistic committee configurations
        let configs = vec![
            (4, 1),    // Minimum viable committee
            (4, 10),   // Small committee
            (7, 50),   // Medium committee
            (10, 100), // Large committee
            (13, 200), // Very large committee
        ];

        for (committee_size, num_validators) in configs {
            let committee = create_test_committee_info(committee_size, num_validators);
            let rate = calculate_message_rate_for_topic(&[committee]);

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
        let committee = create_test_committee_info(1, 1);
        let rate = calculate_message_rate_for_topic(&[committee]);

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
        let committee_size = 4;
        let num_validators = 10;

        let attestation_duties =
            expected_committee_duties_per_epoch_due_to_attestation(num_validators);
        let sync_duties = expected_single_sc_committee_duties_per_epoch(num_validators);

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
        let aggregator_duties = num_validators as f64 * AGGREGATOR_PROBABILITY;
        let proposal_duties = num_validators as f64 * SLOTS_PER_EPOCH * PROPOSAL_PROBABILITY;
        let sync_agg_duties = num_validators as f64 * SLOTS_PER_EPOCH * SYNC_COMMITTEE_AGG_PROB;

        assert!(aggregator_duties >= 0.0 && aggregator_duties.is_finite());
        assert!(proposal_duties >= 0.0 && proposal_duties.is_finite());
        assert!(sync_agg_duties >= 0.0 && sync_agg_duties.is_finite());
    }
}

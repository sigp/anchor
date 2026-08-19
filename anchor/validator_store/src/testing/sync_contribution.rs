//! Tests for pre-Boole post-consensus sync contribution preparation and batching.

use std::{collections::HashMap, time::Duration};

use bls::{AggregateSignature, Signature};
use fork::Fork;
use futures::StreamExt;
use signature_collector::{SignatureRequester, SyncCommitteeBatchEntry};
use ssv_types::{
    OperatorId, ValidatorIndex,
    consensus::{
        BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION, Contribution, ContributionWrapper, Contributions,
        ProposerConsensusData, ValidatorDuty,
    },
    partial_sig::PartialSignatureKind,
};
use ssz::Encode;
use ssz_types::VariableList;
use types::{
    ContributionAndProof, Domain, EthSpec, ForkName, Hash256, MainnetEthSpec, SignedRoot, Slot,
    SyncCommitteeContribution, SyncSelectionProof, SyncSubnetId,
};
use validator_store::{ContributionToSign, ValidatorStore};

use super::common::*;
use crate::{
    AggregationAssignments, CollectionMode, ContributionWaiter, SpecificError,
    prepare_decided_sync_contributions, sync_committee_collection_mode,
};

const OUR_OPERATOR_ID: OperatorId = OperatorId(1);
const AGGREGATOR_INDEX: u64 = 42;
const DOMAIN_HASH: Hash256 = Hash256::repeat_byte(0xDD);

#[derive(Clone, Copy, Debug)]
struct ContributionSpec {
    subnet: u64,
    block_root_byte: u8,
}

fn contribution(spec: ContributionSpec) -> Contribution<MainnetEthSpec> {
    Contribution {
        selection_proof_sig: Signature::empty(),
        contribution: SyncCommitteeContribution {
            slot: Slot::new(TEST_SLOT),
            beacon_block_root: Hash256::repeat_byte(spec.block_root_byte),
            subcommittee_index: spec.subnet,
            aggregation_bits: Default::default(),
            signature: AggregateSignature::infinity(),
        },
    }
}

fn message(spec: ContributionSpec) -> ContributionAndProof<MainnetEthSpec> {
    let contribution = contribution(spec);
    ContributionAndProof {
        aggregator_index: AGGREGATOR_INDEX,
        contribution: contribution.contribution,
        selection_proof: contribution.selection_proof_sig,
    }
}

fn decided(specs: &[ContributionSpec]) -> Contributions<MainnetEthSpec> {
    Contributions::new(
        specs
            .iter()
            .copied()
            .map(contribution)
            .map(ContributionWrapper::from)
            .collect(),
    )
    .expect("test contributions should fit the bounded decided value")
}

fn expected_descriptor(
    entries: &[(ContributionSpec, usize)],
    domain_hash: Hash256,
) -> Vec<SyncCommitteeBatchEntry> {
    let mut descriptor = entries
        .iter()
        .map(|(spec, multiplicity)| SyncCommitteeBatchEntry {
            subnet_id: SyncSubnetId::new(spec.subnet),
            signing_root: message(*spec).signing_root(domain_hash),
            multiplicity: *multiplicity,
        })
        .collect::<Vec<_>>();
    descriptor.sort_unstable_by_key(|entry| (u64::from(entry.subnet_id), entry.signing_root));
    descriptor
}

#[test]
fn decided_contribution_preparation_cases() {
    struct Case {
        name: &'static str,
        decided: Vec<ContributionSpec>,
        callback_subnet: u64,
        expected_entries: Vec<(ContributionSpec, usize)>,
    }

    let a = ContributionSpec {
        subnet: 0,
        block_root_byte: 0x10,
    };
    let b = ContributionSpec {
        subnet: 1,
        block_root_byte: 0x11,
    };
    let c = ContributionSpec {
        subnet: 2,
        block_root_byte: 0x12,
    };
    let d = ContributionSpec {
        subnet: 3,
        block_root_byte: 0x13,
    };
    let same_subnet_first = ContributionSpec {
        subnet: 1,
        block_root_byte: 0x21,
    };
    let same_subnet_second = ContributionSpec {
        subnet: 1,
        block_root_byte: 0x20,
    };

    let cases = [
        Case {
            name: "one root",
            decided: vec![a],
            callback_subnet: 0,
            expected_entries: vec![(a, 1)],
        },
        Case {
            name: "two roots in reverse subnet order",
            decided: vec![b, a],
            callback_subnet: 1,
            expected_entries: vec![(a, 1), (b, 1)],
        },
        Case {
            name: "four shuffled roots",
            decided: vec![d, b, a, c],
            callback_subnet: 2,
            expected_entries: vec![(a, 1), (b, 1), (c, 1), (d, 1)],
        },
        Case {
            name: "same four roots in another permutation",
            decided: vec![c, a, d, b],
            callback_subnet: 2,
            expected_entries: vec![(a, 1), (b, 1), (c, 1), (d, 1)],
        },
        Case {
            name: "identical entry multiplicity",
            decided: vec![b, a, b],
            callback_subnet: 1,
            expected_entries: vec![(a, 1), (b, 2)],
        },
        Case {
            name: "decided subnet without a local callback",
            decided: vec![a, d],
            callback_subnet: 0,
            expected_entries: vec![(a, 1), (d, 1)],
        },
        Case {
            name: "two distinct roots on one subnet",
            decided: vec![same_subnet_first, same_subnet_second],
            callback_subnet: 1,
            expected_entries: vec![(same_subnet_first, 1), (same_subnet_second, 1)],
        },
    ];

    for case in cases {
        let first_matching = case
            .decided
            .iter()
            .copied()
            .find(|spec| spec.subnet == case.callback_subnet)
            .expect("valid case should contain its callback subnet");
        let prepared = prepare_decided_sync_contributions(
            decided(&case.decided),
            SyncSubnetId::new(case.callback_subnet),
            AGGREGATOR_INDEX,
            DOMAIN_HASH,
        )
        .unwrap_or_else(|error| panic!("{} should prepare: {error:?}", case.name));
        let expected = expected_descriptor(&case.expected_entries, DOMAIN_HASH);

        assert_eq!(
            prepared.callback_message,
            message(first_matching),
            "{}",
            case.name
        );
        assert_eq!(
            prepared.callback_signing_root,
            message(first_matching).signing_root(DOMAIN_HASH),
            "{}",
            case.name
        );
        assert_eq!(prepared.descriptor, expected, "{}", case.name);

        let total_multiplicity = expected
            .iter()
            .map(|entry| entry.multiplicity)
            .sum::<usize>();
        match sync_committee_collection_mode(
            SyncSubnetId::new(case.callback_subnet),
            prepared.descriptor,
        ) {
            CollectionMode::SingleValidator if total_multiplicity == 1 => {}
            CollectionMode::SingleValidatorBatch {
                subnet_id,
                descriptor,
            } if total_multiplicity > 1 => {
                assert_eq!(subnet_id, SyncSubnetId::new(case.callback_subnet));
                assert_eq!(descriptor, expected);
            }
            _ => panic!("{} selected the wrong collection mode", case.name),
        }
    }
}

#[test]
fn decided_contribution_preparation_preserves_no_data_agreed() {
    let result = prepare_decided_sync_contributions(
        decided(&[ContributionSpec {
            subnet: 0,
            block_root_byte: 0x10,
        }]),
        SyncSubnetId::new(1),
        AGGREGATOR_INDEX,
        DOMAIN_HASH,
    );

    assert!(matches!(result, Err(SpecificError::NoDataAgreed)));
}

fn fixed_consensus_data(
    validator_pubkey: bls::PublicKeyBytes,
    validator_index: ValidatorIndex,
    decided: &Contributions<MainnetEthSpec>,
) -> ProposerConsensusData {
    ProposerConsensusData {
        duty: ValidatorDuty {
            r#type: BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION,
            pub_key: validator_pubkey,
            slot: Slot::new(TEST_SLOT),
            validator_index,
            committee_index: 0,
            committee_length: 0,
            committees_at_slot: 0,
            validator_committee_index: AGGREGATOR_INDEX,
            validator_sync_committee_indices: Default::default(),
        },
        version: ForkName::Altair.into(),
        data_ssz: VariableList::new(decided.as_ssz_bytes())
            .expect("decided contribution bytes should fit proposer consensus data"),
    }
}

fn contribution_to_sign(
    validator_pubkey: bls::PublicKeyBytes,
    spec: ContributionSpec,
) -> ContributionToSign<MainnetEthSpec> {
    let data = contribution(spec);
    ContributionToSign {
        aggregator_index: AGGREGATOR_INDEX,
        aggregator_pubkey: validator_pubkey,
        contribution: data.contribution,
        selection_proof: SyncSelectionProof::from(data.selection_proof_sig),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pre_boole_callbacks_use_the_complete_fixed_decided_descriptor() {
    let committee = create_committee_setup(
        &[OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)],
        1,
        AGGREGATOR_INDEX as usize,
    );
    let validator = committee.validators[0].clone();
    let callback_a = ContributionSpec {
        subnet: 0,
        block_root_byte: 0x31,
    };
    let callback_b = ContributionSpec {
        subnet: 1,
        block_root_byte: 0x32,
    };
    let phantom = ContributionSpec {
        subnet: 3,
        block_root_byte: 0x33,
    };
    let fixed_decided = decided(&[phantom, callback_b, callback_a]);
    let validator_index = validator
        .index
        .expect("test validator should have an index");
    let fixed_value = fixed_consensus_data(validator.public_key, validator_index, &fixed_decided);
    let harness = ValidatorStoreTestHarness::new_with_fork_and_consensus(
        vec![committee],
        OUR_OPERATOR_ID,
        Fork::Alan,
        MockConsensusDecider::fixed_after_barrier(&fixed_value, 2),
    );
    let new_executions =
        harness
            .validator_store
            .update_aggregation_assignments(AggregationAssignments {
                slot: Slot::new(TEST_SLOT),
                aggregator_committees: HashMap::new(),
                multi_sync_aggregators: HashMap::from([(
                    validator.public_key,
                    ContributionWaiter::new(2),
                )]),
                consensus_data_by_ssv_committee: HashMap::new(),
            });
    assert!(
        new_executions.is_empty(),
        "pre-Boole assignments without consensus data must not register aggregate executions"
    );

    let stream = harness
        .validator_store
        .sign_sync_committee_contributions(vec![
            contribution_to_sign(validator.public_key, callback_b),
            contribution_to_sign(validator.public_key, callback_a),
        ]);
    let results = tokio::time::timeout(Duration::from_secs(5), stream.collect::<Vec<_>>())
        .await
        .expect("concurrent contribution callbacks should complete promptly");
    let mut signed = results
        .into_iter()
        .next()
        .expect("pre-Boole signing should produce one streamed result")
        .expect("pre-Boole signing should succeed");
    signed.sort_by_key(|item| item.message.contribution.subcommittee_index);
    assert_eq!(signed.len(), 2);
    assert_eq!(signed[0].message, message(callback_a));
    assert_eq!(signed[1].message, message(callback_b));

    let epoch = Slot::new(TEST_SLOT).epoch(MainnetEthSpec::slots_per_epoch());
    let domain_hash = harness
        .validator_store
        .get_domain(epoch, Domain::ContributionAndProof);
    let expected = expected_descriptor(
        &[(callback_a, 1), (callback_b, 1), (phantom, 1)],
        domain_hash,
    );
    let mut captured = harness.captured_calls.lock();
    captured.sort_by_key(|call| match &call.requester {
        SignatureRequester::SingleValidatorBatch { subnet_id, .. } => u64::from(*subnet_id),
        other => panic!("expected post-consensus batch requester, got {other:?}"),
    });
    assert_eq!(captured.len(), 2);
    for (call, callback) in captured.iter().zip([callback_a, callback_b]) {
        assert_eq!(call.metadata.kind, PartialSignatureKind::PostConsensus);
        assert_eq!(call.metadata.role, ssv_types::msgid::Role::SyncCommittee);
        assert_eq!(call.metadata.slot, Slot::new(TEST_SLOT));
        assert_eq!(call.validator_pubkey, validator.public_key);
        let expected_root = message(callback).signing_root(domain_hash);
        match &call.requester {
            SignatureRequester::SingleValidatorBatch {
                pubkey,
                subnet_id,
                descriptor,
            } => {
                assert_eq!(*pubkey, validator.public_key);
                assert_eq!(*subnet_id, SyncSubnetId::new(callback.subnet));
                assert_eq!(descriptor, &expected);
            }
            other => panic!("expected post-consensus batch requester, got {other:?}"),
        }
        assert_eq!(call.signing_root, expected_root);
    }
}

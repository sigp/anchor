use std::collections::HashSet;

use serde::{Deserialize, de::IgnoredAny};
use ssv_types::{OperatorId, get_f, message::SignedSSVMessage, quorum_size};

use crate::{
    SpecTest,
    utils::{
        deserializers::{deserialize_base64_list, deserialize_base64_or_null},
        dtos::RawSSVMessage,
        error_codes,
    },
};

/// Fixture container for the signed message in `CommitteeMemberTest`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct FixtureMessage {
    #[serde(deserialize_with = "deserialize_base64_list")]
    signatures: Vec<Vec<u8>>,
    #[serde(rename = "OperatorIDs")]
    operator_ids: Vec<u64>,
    #[serde(rename = "SSVMessage")]
    ssv_message: RawSSVMessage,
    #[serde(deserialize_with = "deserialize_base64_or_null", default)]
    full_data: Option<Vec<u8>>,
}

/// `committee` is deserialized only for its length; per-operator data isn't needed
/// because quorum is derived from `committee.len()` via `ssv_types::quorum_size`.
/// `faulty_nodes` is asserted equal to `ssv_types::get_f(committee.len())` to detect
/// drift between Anchor's `get_f` and Go's `ComputeF`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct FixtureCommitteeMember {
    committee: Vec<IgnoredAny>,
    faulty_nodes: usize,
}

/// Mirrors Go's `CommitteeMemberTest`. Quorum check reuses `ssv_types::quorum_size`
/// (`2f + 1`), the same helper `Config::quorum_size()` returns and production
/// `Qbft::has_quorum` compares against. This matches Go's `CommitteeMember.GetQuorum`
/// byte-for-byte at all committee sizes.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CommitteeMemberTest {
    committee_member: FixtureCommitteeMember,
    message: FixtureMessage,
    expected_has_quorum: bool,
    expected_full_committee: bool,
    expected_error_code: i64,
}

impl SpecTest for CommitteeMemberTest {
    fn run(&self) -> Result<(), String> {
        let actual_code = self
            .build_signed_message()
            .err()
            .unwrap_or(error_codes::NO_ERROR);
        error_codes::assert_error_code(self.expected_error_code, actual_code)?;

        // Quorum thresholds use deduplicated signers (matches Go's
        // `GetUniqueMessageSignersCount` behavior).
        let unique_signers: HashSet<u64> = self.message.operator_ids.iter().copied().collect();
        let unique_count = unique_signers.len();
        let committee_size = self.committee_member.committee.len();

        // Verify the fixture's stored `FaultyNodes` agrees with Anchor's derivation.
        // Go's `CommitteeMember.HasQuorum` uses the stored field directly, so this
        // catches drift between `ssv_types::get_f` and Go's `ComputeF`.
        let derived_f = get_f(committee_size);
        if self.committee_member.faulty_nodes != derived_f {
            return Err(format!(
                "FaultyNodes mismatch: fixture={}, get_f({committee_size})={derived_f}",
                self.committee_member.faulty_nodes,
            ));
        }

        let required_quorum = quorum_size(committee_size);

        let has_quorum = unique_count >= required_quorum;
        if has_quorum != self.expected_has_quorum {
            return Err(format!(
                "has_quorum: expected {}, got {has_quorum} \
                 (unique={unique_count}, committee_size={committee_size}, \
                 required={required_quorum})",
                self.expected_has_quorum,
            ));
        }

        let full_committee = unique_count == committee_size;
        if full_committee != self.expected_full_committee {
            return Err(format!(
                "full_committee: expected {}, got {full_committee} \
                 (unique={unique_count}, committee={committee_size})",
                self.expected_full_committee,
            ));
        }

        Ok(())
    }
}

impl CommitteeMemberTest {
    /// Run the fixture through the production `SignedSSVMessage::new()` validator.
    /// Returns the mapped Go error code on failure.
    fn build_signed_message(&self) -> Result<SignedSSVMessage, i64> {
        let ssv_message = (&self.message.ssv_message)
            .try_into()
            .map_err(|_: String| error_codes::UNMAPPED_ERROR_CODE)?;

        let signatures = self
            .message
            .signatures
            .iter()
            .map(|sig| {
                <[u8; 256]>::try_from(sig.as_slice()).map_err(|_| error_codes::EMPTY_SIGNATURE)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let operator_ids = self
            .message
            .operator_ids
            .iter()
            .copied()
            .map(OperatorId)
            .collect();

        SignedSSVMessage::new(
            signatures,
            operator_ids,
            ssv_message,
            self.message.full_data.clone().unwrap_or_default(),
        )
        .map_err(|e| error_codes::signed_ssv_message_error_code(&e))
    }
}

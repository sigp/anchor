use crate::{SpecTest, SpecTestType, types::TypesSpecTestType, types::types_deserializers::*};
use ::ssz::Decode;
use serde::Deserialize;
use ssv_types::SpecShare;
use tree_hash::TreeHash;
use types::Hash256;

// Share encoding test
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareEncodingTest {
    #[serde(rename = "Name")]
    pub name: String,

    #[serde(rename = "Data", deserialize_with = "deserialize_base64_to_bytes")]
    pub data: Vec<u8>,

    #[serde(
        rename = "ExpectedRoot",
        deserialize_with = "deserialize_bytes_to_hash256"
    )]
    pub expected_root: Hash256,
}

impl SpecTest for ShareEncodingTest {
    fn name(&self) -> &str {
        &self.name
    }

    fn setup(&mut self) {
        // Setup any required test state
    }

    fn run(&self) -> bool {
        // Parse the SSZ-encoded Share data
        let share = match SpecShare::from_ssz_bytes(&self.data) {
            Ok(share) => {
                println!("✅ Successfully decoded Share from SSZ data: {}", self.name);
                println!("   ValidatorIndex: {}", share.validator_index);
                println!(
                    "   ValidatorPubKey: {} bytes",
                    share.validator_pub_key.len()
                );
                println!("   SharePubKey: {} bytes", share.share_pub_key.len());
                println!("   Committee members: {}", share.committee.len());
                println!("   DomainType: {} bytes", share.domain_type.len());
                println!(
                    "   FeeRecipientAddress: {} bytes",
                    share.fee_recipient_address.len()
                );
                println!("   Graffiti: {} bytes", share.graffiti.len());
                share
            }
            Err(e) => {
                println!(
                    "❌ Failed to decode Share from SSZ data: {} - Error: {:?}",
                    self.name, e
                );
                return false;
            }
        };

        // Structure validation is enforced by the Vector/List types at compile time

        // Calculate the TreeHash root
        let calculated_root = share.tree_hash_root();
        let expected_root_bytes: [u8; 32] = self.expected_root.0;

        if calculated_root != expected_root_bytes {
            println!("❌ TreeHash root mismatch for test: {}", self.name);
            println!("   Expected: {:?}", expected_root_bytes);
            println!("   Calculated: {:?}", calculated_root);
            return false;
        }

        println!("✅ Share encoding test passed: {}", self.name);
        println!("   TreeHash root matches expected value");
        println!("   Root: {:?}", calculated_root);

        true
    }

    fn test_type() -> SpecTestType {
        SpecTestType::Types(TypesSpecTestType::ShareEncoding)
    }
}

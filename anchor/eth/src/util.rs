use super::sync::MAX_OPERATORS;
use crate::event_processor::BeaconClient;
use alloy::primitives::{keccak256, Address};
use rand::Rng;
use reqwest::Client;
use serde::Deserialize;
use ssv_types::Share;
use ssv_types::{ClusterId, OperatorId, ValidatorIndex, ValidatorMetadata};
use std::collections::HashSet;
use std::str::FromStr;
use types::{Graffiti, Hash256, PublicKey, Signature};

use std::time::Duration;

// phase0.SignatureLength
const SIGNATURE_LENGTH: usize = 96;
// phase0.PublicKeyLength
const PUBLIC_KEY_LENGTH: usize = 48;
// Length of an encrypted key
const ENCRYPTED_KEY_LENGTH: usize = 256; // Leng

// Api endpoint to fetch index of a validator
const INDEX_ENDPOINT: &str = "/eth/v1/beacon/states/head/validators/";

#[derive(Deserialize, Default)]
struct ValidatorResponse {
    data: ValidatorInfo,
}

#[derive(Deserialize, Default)]
struct ValidatorInfo {
    index: String,
}

impl BeaconClient {
    // Initialize a new client with default settings
    pub fn new(base_url: &str) -> Self {
        // Configure the client with reasonable defaults
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("Failed to create HTTP client");

        BeaconClient {
            client,
            base_url: base_url.to_string(),
        }
    }

    // Method to get validator information
    pub async fn get_validator_index(&self, pubkey: &str) -> usize {
        // Combine base URL with the specific validator endpoint
        let url = format!(
            "{}/eth/v1/beacon/states/head/validators/{}",
            self.base_url, pubkey
        );

        // Handle the request's Result explicitly since Response doesn't implement Default
        let response = match self.client.get(&url).send().await {
            Ok(resp) => resp,
            Err(_) => return 0,
        };

        // Then handle JSON parsing, using default if it fails
        let validator_response = response
            .json::<ValidatorResponse>()
            .await
            .unwrap_or_default();

        // Finally parse the index to usize, defaulting to 0 if it fails
        validator_response.data.index.parse().unwrap_or(0)
    }
}

// Parses shares from a ValidatorAdded event
// Event contains a bytes stream of the form
// [signature | public keys | encrypted keys].
pub fn parse_shares(
    shares: Vec<u8>,
    operator_ids: &[OperatorId],
    cluster_id: &ClusterId,
    validator_pubkey: &PublicKey,
) -> Result<(Vec<u8>, Vec<Share>), String> {
    let operator_count = operator_ids.len();

    // Calculate offsets for different components within the shares
    let signature_offset = SIGNATURE_LENGTH;
    let pub_keys_offset = PUBLIC_KEY_LENGTH * operator_count + signature_offset;
    let shares_expected_length = ENCRYPTED_KEY_LENGTH * operator_count + pub_keys_offset;

    // Validate total length of shares
    if shares_expected_length != shares.len() {
        return Err(format!(
            "Share data has invalid length: expected {}, got {}",
            shares_expected_length,
            shares.len()
        ));
    }

    // Extract components
    let signature = shares[..signature_offset].to_vec();
    let share_public_keys = split_bytes(
        &shares[signature_offset..pub_keys_offset],
        PUBLIC_KEY_LENGTH,
    );
    let encrypted_keys: Vec<Vec<u8>> =
        split_bytes(&shares[pub_keys_offset..], ENCRYPTED_KEY_LENGTH);

    // Create the shares from the share public keys and the encrypted private keys
    let shares: Vec<Share> = share_public_keys
        .into_iter()
        .zip(encrypted_keys)
        .zip(operator_ids)
        .map(|((public, encrypted), operator_id)| {
            // Add 0x prefix to the hex encoded public key
            let public_key_hex = format!("0x{}", hex::encode(&public));

            // Create public key
            let share_pubkey = PublicKey::from_str(&public_key_hex)
                .map_err(|e| format!("Failed to create public key: {}", e))?;

            // Convert encrypted key into fixed array
            let encrypted_array: [u8; 256] = encrypted
                .try_into()
                .map_err(|_| "Encrypted key has wrong length".to_string())?;

            Ok(Share {
                validator_pubkey: validator_pubkey.clone(),
                operator_id: *operator_id,
                cluster_id: *cluster_id,
                share_pubkey,
                encrypted_private_key: encrypted_array,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok((signature, shares))
}

// Splits a byte slice into chunks of specified size
fn split_bytes(data: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
    data.chunks(chunk_size)
        .map(|chunk| chunk.to_vec())
        .collect()
}

// Fetch the metadata for a validator from the beacon chain
pub fn fetch_validator_metadata(
    public_key: &PublicKey,
    cluster_id: &ClusterId,
) -> Result<ValidatorMetadata, String> {
    // Default Anchor-SSV Graffiti
    let mut bytes = [0u8; 32];
    bytes[..10].copy_from_slice(b"Anchor-SSV");

    Ok(ValidatorMetadata {
        index: ValidatorIndex(rand::thread_rng().gen_range(0..100)), // fetch from chain?
        public_key: public_key.clone(),
        graffiti: Graffiti::from(bytes),
        cluster_id: *cluster_id,
    })
}

// Verify that the signature over the share data is correct
pub fn verify_signature(
    signature: Vec<u8>,
    nonce: u16,
    owner: &Address,
    public_key: &PublicKey,
) -> bool {
    // Hash the owner and nonce concatinated
    let data = format!("{}:{}", owner, nonce);
    let hash = keccak256(data);

    // Deserialize the signature
    let signature = Signature::deserialize(&signature).expect("Failed to deserialize signature");

    // Verify the signature against the message
    signature.verify(public_key, hash)
}

// Perform basic verification on the operator set
pub fn validate_operators(operator_ids: &[OperatorId]) -> Result<(), String> {
    let num_operators = operator_ids.len();

    // make sure there is a valid number of operators
    if num_operators > MAX_OPERATORS {
        return Err(format!(
            "Validator has too many operators: {}",
            num_operators
        ));
    }
    if num_operators == 0 {
        return Err("Validator has no operators".to_string());
    }

    // make sure count is valid
    let threshold = (num_operators - 1) / 3;
    if (num_operators - 1) % 3 != 0 || !(1..=4).contains(&threshold) {
        return Err(format!(
            "Given {} operators. Cannot build a 3f+1 quorum",
            num_operators
        ));
    }

    // make sure there are no duplicates
    let mut seen = HashSet::new();
    let are_duplicates = !operator_ids.iter().all(|x| seen.insert(x));
    if are_duplicates {
        return Err("Operator IDs contain duplicates".to_string());
    }

    Ok(())
}

// Compute an identifier from the cluster from the owners and the chosen operators
pub fn compute_cluster_id(owner: Address, mut operator_ids: Vec<u64>) -> ClusterId {
    // Sort the operator IDs
    operator_ids.sort();
    // 20 bytes for the address and num ids * 32 for ids
    let data_size = 20 + (operator_ids.len() * 32);
    let mut data: Vec<u8> = Vec::with_capacity(data_size);

    // Add the address bytes
    data.extend_from_slice(owner.as_slice());

    // Add the operator IDs as 32 byte values
    for id in operator_ids {
        let mut id_bytes = [0u8; 32];
        id_bytes[24..].copy_from_slice(&id.to_be_bytes());
        data.extend_from_slice(&id_bytes);
    }

    // Hash it all
    let hashed_data: [u8; 32] = keccak256(data)
        .as_slice()
        .try_into()
        .expect("Conversion Failed");
    ClusterId(hashed_data)
}

#[cfg(test)]
mod eth_util_tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    // Test to make sure cluster id computation is order independent
    fn test_cluster_id() {
        let owner = Address::random();
        let operator_ids = vec![1, 3, 4, 2];
        let operator_ids_mixed = vec![4, 2, 3, 1];

        let cluster_id_1 = compute_cluster_id(owner, operator_ids);
        let cluster_id_2 = compute_cluster_id(owner, operator_ids_mixed);
        assert_eq!(cluster_id_1, cluster_id_2);
    }

    #[test]
    // Test to make sure a ClusterID matches a current onchain value
    fn test_valid_cluster_id() {
        // https://ssvscan.io/cluster/a3d1e25b31cb6da1b9636568a221b0d7ae1a57a7f14ace5c97d1093ebf6b786c
        let onchain = "a3d1e25b31cb6da1b9636568a221b0d7ae1a57a7f14ace5c97d1093ebf6b786c";
        let operator_ids = vec![62, 256, 259, 282];
        let owner = address!("E1b2308852F0e85D9F23278A6A80131ac8901dBF");
        let cluster_id = compute_cluster_id(owner, operator_ids);
        let cluster_id_hex = hex::encode(*cluster_id);
        assert_eq!(onchain, cluster_id_hex);
    }

    #[test]
    // Test to make sure we can fetch the index of a validator
    fn test_fetch_index() {}

    // Test to make sure we can properly verify signatures
    #[test]
    fn test_sig_verification() {}

    #[test]
    // Ensure that we can properly parse share data into a set of shares
    fn test_parse_shares() {
        // Onchain share data and some other corresponding info
        let share_data = "ab6c91297d2a604d2fc301ad161f99a16baa53e549fd1822acf0f6834450103555b03281d23d0ab7ee944d564f794e040ecd60ad9894747cc6b55ef017876079c1d6aa48595a1791cefc73aa6781c5e26bc644d515e9e9c5bbc8d2b5b173569ba547ba1edf393778d17ad13f2bc8c9b5c2e17b563998a2307b6dddda4d7c6ed3a7f261137fd9c2a81bb1ad1fea6896a8b9719027f01c9b496cf7ade5972e96c94e523e2671662bcfc80d5b6672877de39803d10251d7ecb76794252dea94aa348143c62887bcd62cfb680326c786e22b6a558895f037854e0a70019360c129a788fafe48c18374382cd97a4ea5797bcf982526e76eb89d132e5547f43e9ae9fdf64e061d2f5fcb5bd5ff1de8e7722b53730c6c6a1cc31791fceaabe2e5d79944a7c0d4459ec10153075996e9ef62e4fa9da730873652820c32476c1ddfd10a7b322e67e78759ed9cdec042a09069efc363778f620b3e5ffe01cb1a45bb278768f44342c45736b3a5ccdfbf10b0a10ed26a36af787363398dd776aea98d131738a881739b7e0ee4aa5e280355e2d2254f444ade07c239f5f6870fac2143de480e6ff5e3954d6e441fd16132296960b523bd23fa7b52e357ed03f8201ed4c9b4ed486a66c818e319418c8e34d844b3812f75a74a1607c9bb0eda11c89dbd67858730076e17ed3f6d021c2e57e94e9c3d53e1f6a9c7c2d8373fd5e3340e3a14951e97b7baa5fc1825ba59bb3990f1c607d22756fd178f1a0674d47ee476633f27e961ec3a79b236fb20f863814b47fb9eee75fdbdab99b6901087c41dd31d5320ac3e3c772a8982c64b1c138cbfb968e8a6e59f027bcc53adf2f4f171cbdc6f576dbf313b11485400356865f1f2b0b0533e576d7e3487d5d7d85e8d57aeab4314ec1e49f7647b3eea9a7f1fb805cb944b175c39a2668f96d4cd97afd3dc1258cbaccde6dc5e4b48d4bfd783396505e6f083c5cb3af9e24e90f1eac03f8e8cbc2664b9e6dc81543a1a68973bb03e84f50338ed6c1247447d3a3acef69879900fa9596492cce31130668621f038f365b8b4b1946c95e41e652d868421e574850f5b0b6befb481c93be55c3f9a90f613823942fbd71354ad8202b0121885a0da475d551a86da0c7a983b4d7b403d91adf275b3348fd09b797ccb6be7ebb96efe024588d2f8105e3b7ec5e6cbefd3bb287c82f717597244ea36df07753f0dcc4ce64570fff04447a96cb9f80c6359306c5e45a42e8bbaeb3de9e2ba37aeeed85bcaeb6c61f77c9d26dd4ca853ca09ea8e2e61c675b250c7c6c6c29d7829b3534e0749b9e69b67de569b21f6f0f9a46698b30aad615800aa26ae3629f4b91dfbc3d12cf6b61ed47846b0c0522db60ac41bfc3c4e233bd098180d0257310d58099592d0a5a87e4c6704b64683ee1c746f2a659a01939fbc2b72d196f94452a2b32fa945d1be80a76ba64061bdb73aa23fb83b9e96af949a13e3407a3b37529e79a79814eb172afe4ff56af68417a4191ede4c5c8521ca36c41c0f9e45a960bd32c8a14cb54442e27abf8cf96089736e14340eb017cadf640dbd30014f1802ba6c686e9039f6e5509384a5bfb3f82bef56a4db9778add48a7384d6e25357842a3c591c611908083d420c6e77699793dbf0f1cc597137b48933246c7f5693098a3218312c4ae030dd74b4291e3e1f95702c7f66c22dba7a8ac634e200534c1b6b9c6397c415ab1c448c4eb6481d35250dd83c599cdc05b6e222a4543147e289cf611755dbb1f0968a61c3741a7347db1599b9c4b71e39d4921c7b3bbe018a6a766c7c26fd31e77eb9b727a6a9ca1d72a44317a54e43004f4f42dd5731ed3e83248bc2d5ccef";
        let share_data = hex::decode(share_data).expect("Failed to decode hex string");
        let operator_ids = vec![OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
        let cluster_id = ClusterId([0u8; 32]);
        let pubkey = PublicKey::from_str("0xb1d97447eeb16cffa0464040860db6f12ac0af6a1583a45f4f07fb61e1470f3733f8b7ec8e3c9ff4a9da83086d342ba1").expect("Failed to create public key");

        let (_, shares) = parse_shares(share_data, &operator_ids, &cluster_id, &pubkey)
            .expect("Failed to parse shares");
        assert_eq!(shares.len(), 4);
    }
}

use std::{borrow::Cow, collections::HashSet, num::NonZeroUsize, time::Duration};

use alloy::{
    primitives::{Address, Bytes, Keccak256, keccak256},
    providers::{ProviderBuilder, RootProvider},
    rpc::client::RpcClient,
    transports::{Transport, http::Http, layers::FallbackLayer},
};
use database::NetworkState;
use reqwest::Client;
use sensitive_url::SensitiveUrl;
use ssv_types::{ClusterId, ENCRYPTED_KEY_LENGTH, OperatorId, Share, ValidatorMetadata};
use tower::ServiceBuilder;
use tracing::{debug, error, trace};
use types::{Graffiti, PublicKeyBytes, Signature};

use crate::{error::ExecutionError, sync::MAX_OPERATORS};

// phase0.SignatureLength
const SIGNATURE_LENGTH: usize = 96;
// phase0.PublicKeyLength
const PUBLIC_KEY_LENGTH: usize = 48;

// Parses shares from a ValidatorAdded event
// Event contains a bytes stream of the form
// [signature | public keys | encrypted keys].
pub fn parse_shares<'s>(
    shares: &'s [u8],
    operator_ids: &[OperatorId],
    cluster_id: &ClusterId,
    validator_pubkey: &PublicKeyBytes,
) -> Result<(&'s [u8], Vec<Share>), String> {
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

    // Extract all of the components
    let signature = &shares[..signature_offset];
    let share_public_keys = shares[signature_offset..pub_keys_offset].chunks(PUBLIC_KEY_LENGTH);
    let encrypted_keys = shares[pub_keys_offset..].chunks(ENCRYPTED_KEY_LENGTH);

    // Create the shares from the share public keys and the encrypted private keys
    let shares: Vec<Share> = share_public_keys
        .zip(encrypted_keys)
        .zip(operator_ids)
        .map(|((public, encrypted), operator_id)| {
            // Create public key
            let share_pubkey = PublicKeyBytes::deserialize(public)
                .map_err(|e| format!("Failed to create public key: {e:?}"))?;

            // Convert encrypted key into fixed array
            let encrypted_array: [u8; 256] = encrypted
                .try_into()
                .map_err(|_| "Encrypted key has wrong length".to_string())?;

            Ok(Share {
                validator_pubkey: *validator_pubkey,
                operator_id: *operator_id,
                cluster_id: *cluster_id,
                share_pubkey,
                encrypted_private_key: encrypted_array,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok((signature, shares))
}

// Construct the metadata for the newly added validator
pub fn construct_validator_metadata(
    public_key: &PublicKeyBytes,
    cluster_id: &ClusterId,
) -> Result<ValidatorMetadata, String> {
    // Default Anchor-SSV Graffiti
    let mut bytes = [0u8; 32];
    bytes[..10].copy_from_slice(b"Anchor-SSV");

    // Note: Validator Index is not included in the event log data and it would require a
    // significant refactor to introduce a single non-blocking asynchronous call to fetch this data.
    // For this reason, the population of this field is pushed downstream

    Ok(ValidatorMetadata {
        index: None,
        public_key: *public_key,
        graffiti: Graffiti::from(bytes),
        cluster_id: *cluster_id,
    })
}

// Verify that the signature over the share data is correct
pub fn verify_signature(
    signature: &[u8],
    nonce: u16,
    owner: &Address,
    public_key: &PublicKeyBytes,
) -> bool {
    // Hash the owner and nonce concatinated
    let data = format!("{owner}:{nonce}");
    let hash = keccak256(data);

    // Deserialize the signature
    let signature = match Signature::deserialize(signature) {
        Ok(sig) => sig,
        Err(_) => return false,
    };
    // Decompress the public key
    let public_key = match public_key.decompress() {
        Ok(public_key) => public_key,
        Err(_) => return false,
    };

    // Verify the signature against the message
    signature.verify(&public_key, hash)
}

// Perform basic verification on the operator set
pub fn validate_operators(
    operator_ids: &[OperatorId],
    cluster_id: &ClusterId,
    network_state: &NetworkState,
) -> Result<(), ExecutionError> {
    trace!(cluster_id = ?cluster_id, "Validating operators");

    let num_operators = operator_ids.len();

    // make sure there is a valid number of operators
    if num_operators > MAX_OPERATORS {
        return Err(ExecutionError::InvalidEvent(format!(
            "Failed to validate operators: validator has too many operators: {num_operators}"
        )));
    }
    if num_operators == 0 {
        return Err(ExecutionError::InvalidEvent(
            "Failed to validate operators: validator has no operators".to_string(),
        ));
    }

    // make sure count is valid
    let threshold = (num_operators - 1) / 3;
    if (num_operators - 1) % 3 != 0 || !(1..=4).contains(&threshold) {
        return Err(ExecutionError::InvalidEvent(format!(
            "Given {num_operators} operators. Cannot build a 3f+1 quorum"
        )));
    }

    // make sure there are no duplicates
    let mut seen = HashSet::new();
    let are_duplicates = !operator_ids.iter().all(|x| seen.insert(x));
    if are_duplicates {
        return Err(ExecutionError::InvalidEvent(
            "Operator IDs contain duplicates".to_string(),
        ));
    }

    if operator_ids
        .iter()
        .any(|id| !network_state.operator_exists(id))
    {
        error!(cluster_id = ?cluster_id, "One or more operators do not exist");
        return Err(ExecutionError::Database(
            "One or more operators do not exist".to_string(),
        ));
    }

    Ok(())
}

/// Helper function to parse validator public keys
pub fn parse_validator_pubkey(pubkey: &Bytes) -> Result<PublicKeyBytes, ExecutionError> {
    PublicKeyBytes::deserialize(pubkey).map_err(|e| {
        debug!(
            validator_pubkey = %pubkey,
            error = ?e,
            "Failed to parse validator public key"
        );
        ExecutionError::InvalidEvent(format!("Failed to parse validator public key: {e:?}"))
    })
}

// Compute an identifier from the cluster from the owners and the chosen operators
pub fn compute_cluster_id(owner: Address, operator_ids: &[u64]) -> ClusterId {
    let mut operator_ids = Cow::Borrowed(operator_ids);

    // Sort the operator IDs if necessary
    if !operator_ids.is_sorted() {
        operator_ids.to_mut().sort_unstable();
    }

    let mut hasher = Keccak256::new();
    hasher.update(owner.as_slice());
    for id in operator_ids.as_ref() {
        hasher.update([0; 24]);
        hasher.update(id.to_be_bytes());
    }
    ClusterId(hasher.finalize().0)
}

// Create a http provider with fallbacks
pub fn http_with_timeout_and_fallback(http_urls: &[SensitiveUrl]) -> RootProvider {
    // Base client with connect timeout
    let base = Client::builder()
        .connect_timeout(Duration::from_secs(crate::sync::CONNECT_TIMEOUT))
        .build()
        .expect("Valid client");

    let http_transports: Vec<_> = http_urls
        .iter()
        .map(|u| Http::with_client(base.clone(), u.full.to_owned()))
        .collect();

    provider_from_transports(http_transports)
}

// Create a fallback provider with the provided transports
fn provider_from_transports(
    transports: Vec<impl Transport + std::fmt::Debug + std::clone::Clone>,
) -> RootProvider {
    let fallback_layer = FallbackLayer::default().with_active_transport_count(
        NonZeroUsize::new(transports.len()).expect("Valid fallback layer"),
    );

    // Build the transport service containing the fallback layer and the various transports
    let transport = ServiceBuilder::new()
        .layer(fallback_layer)
        .service(transports);

    // Construct the final client
    let client = RpcClient::builder().transport(transport, false);
    ProviderBuilder::default().on_client(client)
}

#[cfg(test)]
mod eth_util_tests {
    use std::str::FromStr;

    use alloy::primitives::address;

    use super::*;

    #[test]
    // Test to make sure cluster id computation is order independent
    fn test_cluster_id() {
        let owner = Address::random();
        let operator_ids = vec![1, 3, 4, 2];
        let operator_ids_mixed = vec![4, 2, 3, 1];

        let cluster_id_1 = compute_cluster_id(owner, &operator_ids);
        let cluster_id_2 = compute_cluster_id(owner, &operator_ids_mixed);
        assert_eq!(cluster_id_1, cluster_id_2);
    }

    #[test]
    // Test to make sure a ClusterID matches a current onchain value
    fn test_valid_cluster_id() {
        // https://ssvscan.io/cluster/a3d1e25b31cb6da1b9636568a221b0d7ae1a57a7f14ace5c97d1093ebf6b786c
        let onchain = "a3d1e25b31cb6da1b9636568a221b0d7ae1a57a7f14ace5c97d1093ebf6b786c";
        let operator_ids = vec![62, 256, 259, 282];
        let owner = address!("E1b2308852F0e85D9F23278A6A80131ac8901dBF");
        let cluster_id = compute_cluster_id(owner, &operator_ids);
        let cluster_id_hex = hex::encode(*cluster_id);
        assert_eq!(onchain, cluster_id_hex);
    }

    // Test to make sure we can properly verify signatures
    #[test]
    fn test_sig_verification() {
        // random data that was taken from chain
        let owner = address!("382f6ff5b9a29fcf1dd2bf8b86c3234dc7ed2df6");
        let public_key = PublicKeyBytes::from_str("0x94cbce91137bfda4a7638941a68d6b156712bd1ce80e5dc580adc74a445099cbbfb9f97a6c7c89c6a87e28e0657821ac").expect("Failed to create public key");
        let nonce = 8;
        let signature_data = [
            151, 32, 191, 178, 170, 21, 45, 81, 34, 50, 220, 37, 95, 149, 101, 178, 38, 128, 11,
            195, 98, 241, 226, 70, 46, 8, 168, 133, 99, 23, 73, 126, 61, 33, 197, 226, 105, 11,
            134, 248, 226, 127, 60, 108, 102, 109, 148, 135, 16, 76, 114, 132, 123, 186, 148, 147,
            170, 143, 204, 45, 71, 59, 76, 131, 220, 199, 179, 219, 47, 115, 45, 162, 168, 163,
            223, 110, 38, 9, 166, 82, 34, 227, 53, 50, 31, 105, 74, 122, 179, 172, 22, 245, 89, 32,
            214, 69,
        ];
        assert!(verify_signature(
            &signature_data,
            nonce,
            &owner,
            &public_key
        ));

        // make sure that a wrong nonce fails the signature check
        assert!(!verify_signature(
            &signature_data,
            nonce + 1,
            &owner,
            &public_key
        ));
    }

    #[test]
    // Ensure that we can properly parse share data into a set of shares
    fn test_parse_shares() {
        // Onchain share data and some other corresponding info
        let share_data = "ab6c91297d2a604d2fc301ad161f99a16baa53e549fd1822acf0f6834450103555b03281d23d0ab7ee944d564f794e040ecd60ad9894747cc6b55ef017876079c1d6aa48595a1791cefc73aa6781c5e26bc644d515e9e9c5bbc8d2b5b173569ba547ba1edf393778d17ad13f2bc8c9b5c2e17b563998a2307b6dddda4d7c6ed3a7f261137fd9c2a81bb1ad1fea6896a8b9719027f01c9b496cf7ade5972e96c94e523e2671662bcfc80d5b6672877de39803d10251d7ecb76794252dea94aa348143c62887bcd62cfb680326c786e22b6a558895f037854e0a70019360c129a788fafe48c18374382cd97a4ea5797bcf982526e76eb89d132e5547f43e9ae9fdf64e061d2f5fcb5bd5ff1de8e7722b53730c6c6a1cc31791fceaabe2e5d79944a7c0d4459ec10153075996e9ef62e4fa9da730873652820c32476c1ddfd10a7b322e67e78759ed9cdec042a09069efc363778f620b3e5ffe01cb1a45bb278768f44342c45736b3a5ccdfbf10b0a10ed26a36af787363398dd776aea98d131738a881739b7e0ee4aa5e280355e2d2254f444ade07c239f5f6870fac2143de480e6ff5e3954d6e441fd16132296960b523bd23fa7b52e357ed03f8201ed4c9b4ed486a66c818e319418c8e34d844b3812f75a74a1607c9bb0eda11c89dbd67858730076e17ed3f6d021c2e57e94e9c3d53e1f6a9c7c2d8373fd5e3340e3a14951e97b7baa5fc1825ba59bb3990f1c607d22756fd178f1a0674d47ee476633f27e961ec3a79b236fb20f863814b47fb9eee75fdbdab99b6901087c41dd31d5320ac3e3c772a8982c64b1c138cbfb968e8a6e59f027bcc53adf2f4f171cbdc6f576dbf313b11485400356865f1f2b0b0533e576d7e3487d5d7d85e8d57aeab4314ec1e49f7647b3eea9a7f1fb805cb944b175c39a2668f96d4cd97afd3dc1258cbaccde6dc5e4b48d4bfd783396505e6f083c5cb3af9e24e90f1eac03f8e8cbc2664b9e6dc81543a1a68973bb03e84f50338ed6c1247447d3a3acef69879900fa9596492cce31130668621f038f365b8b4b1946c95e41e652d868421e574850f5b0b6befb481c93be55c3f9a90f613823942fbd71354ad8202b0121885a0da475d551a86da0c7a983b4d7b403d91adf275b3348fd09b797ccb6be7ebb96efe024588d2f8105e3b7ec5e6cbefd3bb287c82f717597244ea36df07753f0dcc4ce64570fff04447a96cb9f80c6359306c5e45a42e8bbaeb3de9e2ba37aeeed85bcaeb6c61f77c9d26dd4ca853ca09ea8e2e61c675b250c7c6c6c29d7829b3534e0749b9e69b67de569b21f6f0f9a46698b30aad615800aa26ae3629f4b91dfbc3d12cf6b61ed47846b0c0522db60ac41bfc3c4e233bd098180d0257310d58099592d0a5a87e4c6704b64683ee1c746f2a659a01939fbc2b72d196f94452a2b32fa945d1be80a76ba64061bdb73aa23fb83b9e96af949a13e3407a3b37529e79a79814eb172afe4ff56af68417a4191ede4c5c8521ca36c41c0f9e45a960bd32c8a14cb54442e27abf8cf96089736e14340eb017cadf640dbd30014f1802ba6c686e9039f6e5509384a5bfb3f82bef56a4db9778add48a7384d6e25357842a3c591c611908083d420c6e77699793dbf0f1cc597137b48933246c7f5693098a3218312c4ae030dd74b4291e3e1f95702c7f66c22dba7a8ac634e200534c1b6b9c6397c415ab1c448c4eb6481d35250dd83c599cdc05b6e222a4543147e289cf611755dbb1f0968a61c3741a7347db1599b9c4b71e39d4921c7b3bbe018a6a766c7c26fd31e77eb9b727a6a9ca1d72a44317a54e43004f4f42dd5731ed3e83248bc2d5ccef";
        let share_data = hex::decode(share_data).expect("Failed to decode hex string");
        let operator_ids = vec![OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
        let cluster_id = ClusterId([0u8; 32]);
        let pubkey = PublicKeyBytes::from_str("0xb1d97447eeb16cffa0464040860db6f12ac0af6a1583a45f4f07fb61e1470f3733f8b7ec8e3c9ff4a9da83086d342ba1").expect("Failed to create public key");

        let (_, shares) = parse_shares(&share_data, &operator_ids, &cluster_id, &pubkey)
            .expect("Failed to parse shares");
        assert_eq!(shares.len(), 4);
    }

    #[test]
    // Takes data generated by the keysplitter and verifies its correctness
    fn test_keysplitter() {
        let share_data = "94f4ebe1793f2e84eff9e453b0d66db2cc2a7c85072ed5677dc83227acce4160544980e084a80adabfba9ccc8c96c26313c35432acabb88e46eea17c9cc6e5451eae4a313cf9ef9c581a496463c59a8bd9535f3093ae5202e7799030d7277a238c77c307b0c3bda7ca2ff84ed144a2db283f5478a1dcb98a613d3c0b79f864c9ac90aca17498932beaa1ca5b90818047b8c16f121dc3ae44be832200b45388c3fb38b791886d3dee0cdb9e30c394caa4113e43fa8215889cdd1ee26e31d7c9ad8e02f493fadeeca67e095314e14ff97ef95a9530e3ee961bd6b88aaa980ecaacf3171cd029c0dc7068e83896930f05eeb6b1835b9d26a9b811567bccaf19ee97d2dcdafc3d88f2d0d556aba4c7c3cdcbf994228135dc6bfa4d0dd51067fe0f503f26e63bbeec7e0fcaff670e3d75825deea68313de4085670c8375771e75373ab2c8656fbce23d322d788d70d99942a788d758a2856a6c2826d35d4ffa4bdd9316206ffa3b9af256576fcad3cb4058424c3476553d513125b86062df3f466b758e8924f113d953905f0ba8fa40363c92995a92ff2f22c46419d484b91aa0e0953e1fabed016e3b5bfbcca8c4294dbfef91ea777c9ef4bc501ba71058d31535d696e3d3bac2c689e4adab8444d1b7ee75c4727947c2c55015656bbf3259831d0796c769e98d1b28fdb0d25800bf75afbdf3366535cca9e703fa56e69a6e36fe084170f67562ec9a41b49eeab5782534a01e4acd352eb9c7310689fae53f5f526169ba58f607dd0b314d993fcd0c92b91ac22765f3813c16c8d85a62fc35015b925338fa0a197b5d51a9c13bad2d16b4a62e35d2b4de8827b408cac6804746c2d71cbfcee9ae9270c0c64a06db70f71296bda3af1055177e269222d912373cd58d3f121e6e570ff1cb2ce3d9844f35af5175b3a7f93082f48f8e840874d6029c5a8e4802d204cd13ccf0fa98c5ce2a30f7a8f8eff3bcbb78ea464d450eacbf9b182ffdadf58089d63e81a39dccf41d67c2efe8de8f2774a3d97ff3aaba0c55c22fc47e1daa9ae67c8c63af95c4f398ac046bbb1a1b775882021307607bb50662f3c15bf29384bc2e1f98f5f955d0c12cb126a2828c899b2946075347bf2543566379abd52374fb5b1492d408b178f03c19d431572a2de6fbcd10666b7d3ac5e46a5b25ff72e77ab3c9fd593b0a5638c24d13ce42eb8350c72b440fd2c4fe1c834a04308c89151ed3ed11c272097f7fd831f511afdf616c8fc117c8b97478321667b69d00590d7b4ec08397d2897c8139688c3496fee6dafac57e87ea4cb9ef048e18efd290cb1f48ae45b39d26809ad5ba7150ee9c2abedcfb295cba73baccc832035a8e85994a6f2466a9fb5bbda62a0f7d725db0f7a3fc714dd8b1deb713f732e8fba435ef8f8792b81e411a28dc2753c9bfca11bcb27e548593289fe9406fa4d6d1565c8d9998856a0dcfe7a6c1e207cb22ae19c33e34e318024e24644fd29b51e18c02d53f0705413eb5bd6252459749e1e81be07caabdff0e237d14980d60f5f3bf2dcd980b2e4cfbb3aa936e546d35ed2f097eeeec780fa92c685418c6b510e7c2e226263fe8cb065de7cf6bdea4ed59009d9fd08e4c321630d31b2fb9ca9cd16d4a9ae88ef21b517e805764bec0eca03dbd8b5cf88c561bbde1efc3c3e80080ea3c651942a02075da358dfc6a5769ea83495123569faccd1043e9437b217318c812eab3a1dd58481db3055c29e53b13bb30c47c907021c48833de90d9d977153bcd49ca69c6c772634035d8e339184f8065ee90e1bbb6d19cb4360b4db4cbdeededd91bdec5c54f1ea693037730d238d0aa63c5233207d661abc068e105";
        let share_data = hex::decode(share_data).expect("Failed to decode hex string");
        let operator_ids = vec![OperatorId(1), OperatorId(2), OperatorId(3), OperatorId(4)];
        let cluster_id = ClusterId([0u8; 32]);
        let pubkey = PublicKeyBytes::from_str("0x97e8235ec2174862a8162ef9624f2fb1df82a3a8ef57f72a2a866df37c3da66020b1e4070d0d443ef40198e71afe9493").expect("Failed to create public key");
        let owner = address!("0x000000633b68f5d8d3a86593ebb815b4663bcbe0");
        let nonce = 0;

        let (signature, shares) = parse_shares(&share_data, &operator_ids, &cluster_id, &pubkey)
            .expect("Failed to parse shares");

        assert_eq!(shares.len(), 4);
        assert!(verify_signature(signature, nonce, &owner, &pubkey));
    }
}

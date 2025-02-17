use crate::{base_processing, KeygenError, Onchain, OutputData, EncryptedKeyShare};

// Split the key using onchain data. This takes human error out of the equation and utilizes data scrapped from the chain
// to input the correct operator public keys and owner nonce
pub fn onchain_split(onchain: Onchain) -> Result<Vec<EncryptedKeyShare>, KeygenError> {
    // High level steps
    // Sync all of the data from the chain
    // - should this just be in memory or should we crate keysplitting db?? probs keysplitter
    // Group all of the info into some common struct and then split the key
    // impl display on the key struct to easily write it into a file
    todo!()
}

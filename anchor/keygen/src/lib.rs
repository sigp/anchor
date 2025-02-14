use cli::SharedKeygenOptions;
pub use cli::{Keygen, KeygenSubcommands, Manual, Onchain};
use error::KeygenError;
use std::fs::File;
use types::{Address, PublicKey};
use openssl::pkey::Public;
use openssl::rsa::Rsa;
mod cli;
mod keystore;
mod error;

struct Operator {
    pub id: u32,
    pub public_key: Rsa<Public>,
}

struct KeyshareData {
    nonce: u32,
    owner: Address,
    public_key: PublicKey,
    operators: Vec<u32>
}

// impl display for keyshare {
// }

// Re-direct to manual or onchain keysplitting
pub fn start_keysplitter(keygen: Keygen) {
    let res = match keygen.subcommand {
        KeygenSubcommands::Manual(manual) => manual_split(manual),
        KeygenSubcommands::Onchain(onchain) => onchain_split(onchain),
    };
}

// Perform base processing that is relevant to onchain and manual keysplitting
fn base_processing(shared: &SharedKeygenOptions) -> Result<(), KeygenError>{
    let keystore_file = File::open(shared.keystore_path.clone()).unwrap();
    let keystore = keystore::parse_keystore(keystore_file);

    // read in the keystore file
    Ok(())
}


// Split the key using onchain data. This takes human error out of the equation and utilizes data scrapped from the chain
// to input the correct operator public keys and owner nonce
pub fn onchain_split(onchain: Onchain) -> Result<(), KeygenError>{
    base_processing(&onchain.shared)?;
    // High level steps
    // Sync all of the data from the chain
    // - should this just be in memory or should we crate keysplitting db?? probs keysplitter
    // Group all of the info into some common struct and then split the key
    // impl display on the key struct to easily write it into a file
    Ok(())
}

pub fn manual_split(manual: Manual) -> Result<(), KeygenError> {
    Ok(())
}

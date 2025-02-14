use cli::SharedKeygenOptions;
pub use cli::{Keygen, KeygenSubcommands, Manual, Onchain};
use std::fs::File;
mod cli;
mod keystore;

struct Operator {
    // id
    // publickey
}

struct KeyshareData {
    nonce: u32,
    //owner: Address,
    //public_key PK
    // operators: Vec<u32>
}

pub fn start_keysplitter(keygen: Keygen) {
    match keygen.subcommand {
        KeygenSubcommands::Manual(manual) => manual_split(manual),
        KeygenSubcommands::Onchain(onchain) => onchain_split(onchain),
    }
}

// impl display for keyshare {
// }
fn base_processing(shared: &SharedKeygenOptions) {
    let keystore_file = File::open(shared.keystore_path.clone()).unwrap();
    let keystore = keystore::parse_keystore(keystore_file);


    // read in the keystore file
}

pub fn onchain_split(onchain: Onchain) {
    base_processing(&onchain.shared);
    // High level steps
    // Sync all of the data from the chain
    // - should this just be in memory or should we crate keysplitting db?? probs keysplitter
    // Group all of the info into some common struct and then split the key
    // impl display on the key struct to easily write it into a file
}

pub fn manual_split(manual: Manual) {

}

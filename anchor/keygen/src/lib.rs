pub use cli::{Keygen, KeygenSubcommands, Manual, Onchain};

use crate::manual::manual_split;
use crate::onchain::onchain_split;
use cli::SharedKeygenOptions;
use crypto::extract_keys;
use error::KeygenError;
use std::fs::File;

mod cli;
mod crypto;
mod error;
mod keystore;
mod manual;
mod onchain;
mod util;

// Re-direct to manual or onchain keysplitting
pub fn start_keysplitter(keygen: Keygen) {
    let _res = match keygen.subcommand {
        KeygenSubcommands::Manual(manual) => manual_split(manual),
        KeygenSubcommands::Onchain(onchain) => onchain_split(onchain),
    };

    // todo!() write to the output file
}

// Perform base processing that is relevant to onchain and manual keysplitting
fn base_processing(shared: &SharedKeygenOptions) -> Result<(), KeygenError> {
    let keystore_file = File::open(shared.keystore_path.clone()).unwrap();
    let keystore = keystore::parse_keystore(keystore_file)?;

    let validator_keys = extract_keys(&keystore, &shared.password);
    println!("{:?}", validator_keys.public_key);

    // read in the keystore file
    Ok(())
}

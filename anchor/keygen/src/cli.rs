use clap::Parser;
use std::str::FromStr;

// The menthods of key splitting that the tool supports
// Manual: Manually input all fields for splitting
// Onchain: Scrape onchain data to retrieve information needed for splitting
#[derive(Parser, Clone, Debug)]
#[clap(name = "keygen", about = "SSV Keysplitting Tool")]
pub struct Keygen {
    #[clap(subcommand)]
    pub subcommand: KeygenSubcommands,
}

#[derive(Parser, Clone, Debug)]
pub enum KeygenSubcommands {
    Onchain(Onchain),
    Manual(Manual),
}

// Options for onchain splitting
#[derive(Parser, Clone, Debug)]
#[clap(name = "onchain", about = "Utilize onchain data to split the key")]
pub struct Onchain {
    #[clap(flatten)]
    pub shared: SharedKeygenOptions,

    #[clap(long, help = "RPC endpoint to access L1 data", value_name = "ENDPOINT")]
    pub rpc: String,
}

// Options for manual splitting
#[derive(Parser, Clone, Debug)]
#[clap(name = "manual", about = "Split the key by manually providing data")]
pub struct Manual {
    #[clap(flatten)]
    pub shared: SharedKeygenOptions,

    #[clap(long, help = "Nonce for the owner address", value_name = "NONCE")]
    pub nonce: u32,
    // todo!() the keys
    // keys
}

// Options that are releveant to both onchain and manual keysplitting
#[derive(Parser, Clone, Debug)]
pub struct SharedKeygenOptions {
    #[clap(
        long,
        help = "Path to the validator keystore file",
        value_name = "PATH"
    )]
    pub keystore_path: String,

    #[clap(
        long,
        help = "Password for the validator keystore",
        value_name = "PASSWORD"
    )]
    pub password: String,

    #[clap(
        long,
        help = "EOA address that owns the validator",
        value_name = "ADDRESS"
    )]
    pub owner: String,

    #[clap(long, help = "Path for output", value_name = "OUTPUT PATH")]
    pub output_path: String,

    #[clap(long, help = "Operators to split key among", value_name = "IDS")]
    pub operators: OperatorIds,
}

// Operators that are going to be part of the committee
#[derive(Debug, Clone)]
pub struct OperatorIds(pub Vec<u32>);

// Enforce that the user can only enter 4, 7, 10, or 13 operators
impl FromStr for OperatorIds {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // First, parse all the numbers from the input string
        let numbers: Vec<u32> = s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|num| num.parse::<u32>())
            .collect::<Result<Vec<u32>, _>>()
            .map_err(|e| format!("Failed to parse number: {}", e))?;

        // Now validate the length matches our requirements
        match numbers.len() {
            4 | 7 | 10 | 13 => Ok(OperatorIds(numbers)),
            len => Err(format!(
                "Invalid number of operators: {}. Must be 4, 7, 10, or 13 numbers",
                len
            )),
        }
    }
}

use std::{env, fs};

use clap::{Command, CommandFactory};
use client::cli::{
    ExternalApis, HttpApi, MetricsOptions, NetworkOptions, PayloadBuildingOptions, SecurityOptions,
};
use keygen::Keygen;
use keysplit::{Manual, Onchain, SharedKeygenOptions};
use logging::FileLoggingFlags;
use tracing::info;

pub struct DocGenerator;

impl DocGenerator {
    pub fn generate_and_write_docs() -> Result<(), Box<dyn std::error::Error>> {
        let current_dir = env::current_dir()?;
        let docs_path = current_dir.join("docs").join("docs").join("pages");

        fs::create_dir_all(&docs_path)?;

        let node_path = docs_path.join("cli-node.mdx");
        let keygen_path = docs_path.join("cli-keygen.mdx");
        let keysplit_path = docs_path.join("cli-keysplit.mdx");

        fs::write(&node_path, Self::generate_node_docs())?;
        fs::write(&keygen_path, Self::generate_keygen_docs())?;
        fs::write(&keysplit_path, Self::generate_keysplit_docs())?;

        info!("Documentation files generated:");
        info!("- {}", node_path.display());
        info!("- {}", keygen_path.display());
        info!("- {}", keysplit_path.display());

        Ok(())
    }

    /// Generate node documentation
    fn generate_node_docs() -> String {
        let mut doc = String::new();

        doc.push_str("# Node Command\n\n");
        doc.push_str("The `node` command starts the anchor client as a SSV operator node.\n\n");
        doc.push_str("```bash\n");
        doc.push_str("anchor node [OPTIONS]\n");
        doc.push_str("```\n\n");
        doc.push_str("## Options\n\n");

        let sections = [
            ("External APIs", ExternalApis::command()),
            ("HTTP API", HttpApi::command()),
            ("Metrics Options", MetricsOptions::command()),
            ("Network Options", NetworkOptions::command()),
            ("Security Options", SecurityOptions::command()),
            (
                "Payload Building Options",
                PayloadBuildingOptions::command(),
            ),
            ("Logging Options", FileLoggingFlags::command()),
        ];

        for (section_name, command) in sections {
            doc.push_str(&Self::build_options_table(command, section_name));
        }

        doc.push_str("## Examples\n\n");
        doc.push_str("```bash\n");
        doc.push_str("anchor node \\\n");
        doc.push_str("  --network hoodi \\\n");
        doc.push_str("  --datadir /data/anchor \\\n");
        doc.push_str(
            "  --beacon-nodes https://beacon1.example.com,https://beacon2.example.com \\\n",
        );
        doc.push_str(
            "  --execution-rpc https://execution1.example.com,https://execution2.example.com \\\n",
        );
        doc.push_str("  --execution-ws wss://execution1.example.com \\\n");
        doc.push_str("  --listen-address 10.0.0.10 \\\n");
        doc.push_str("  --port 9100 \\\n");
        doc.push_str("  --http \\\n");
        doc.push_str("  --http-address 127.0.0.1 \\\n");
        doc.push_str("  --http-port 9200 \\\n");
        doc.push_str("  --unencrypted-http-transport \\\n");
        doc.push_str("  --metrics \\\n");
        doc.push_str("  --metrics-address 127.0.0.1 \\\n");
        doc.push_str("  --metrics-port 9300 \\\n");
        doc.push_str("  --password-file /path/to/your/password\n");
        doc.push_str("```\n");

        doc
    }

    /// Generate keygen documentation
    fn generate_keygen_docs() -> String {
        let mut doc = String::new();

        doc.push_str("# Keygen Command\n\n");
        doc.push_str(
            "The `keygen` command generates RSA keys for SSV operator identification.\n\n",
        );
        doc.push_str("```bash\n");
        doc.push_str("anchor keygen [OPTIONS]\n");
        doc.push_str("```\n\n");

        doc.push_str(&Self::build_options_table(Keygen::command(), "Options"));

        doc.push_str("## Examples\n\n");
        doc.push_str("### Basic key generation\n\n");
        doc.push_str("This will create an unencrypted `private_key.txt` file containing the newly generated ");
        doc.push_str(
            "private key and a `public_key.txt` file with the BASE64 encoded public key used for ",
        );
        doc.push_str("registering the operator.\n\n");
        doc.push_str("```bash\n");
        doc.push_str("anchor keygen\n");
        doc.push_str("```\n\n");

        doc.push_str("### Encrypted key generation\n\n");
        doc.push_str("This will create a `encrypted_private_key.json` file encrypted with the provided password ");
        doc.push_str("and a `public_key.txt` file with the BASE64 encoded public key used for registering the ");
        doc.push_str("operator. The password must be provided via `--password-file` or interactively when running ");
        doc.push_str("Anchor.\n\n");
        doc.push_str("```bash\n");
        doc.push_str("anchor keygen --encrypt --output-path /path/to/keys\n");
        doc.push_str("```\n\n");

        doc.push_str("## Key Storage\n\n");
        doc.push_str(
            "Anchor will look for the key file in the default directory `~/.anchor/{network}`, ",
        );
        doc.push_str("or the directory specified by `--datadir`.\n");

        doc
    }

    /// Generate keysplit documentation
    fn generate_keysplit_docs() -> String {
        let mut doc = String::new();

        doc.push_str("# Keysplit Command\n\n");
        doc.push_str("The `keysplit` command is used to split validator keys for distributed validation on the SSV network.\n\n");
        doc.push_str("```bash\n");
        doc.push_str("anchor keysplit <SUBCOMMAND> [OPTIONS]\n");
        doc.push_str("```\n\n");

        doc.push_str("## Subcommands\n\n");
        doc.push_str("- `manual` - Split keys with manually provided operator data\n");
        doc.push_str("- `onchain` - Split keys using operator data from the blockchain\n\n");

        doc.push_str(&Self::build_options_table(
            SharedKeygenOptions::command(),
            "Shared Options",
        ));

        doc.push_str("## Manual Keysplit Subcommand\n\n");
        doc.push_str("```bash\n");
        doc.push_str("anchor keysplit manual [OPTIONS]\n");
        doc.push_str("```\n\n");
        doc.push_str(&Self::build_options_table(
            Manual::command(),
            "Manual-specific Options",
        ));

        doc.push_str("## Onchain Keysplit Subcommand\n\n");
        doc.push_str("```bash\n");
        doc.push_str("anchor keysplit onchain [OPTIONS]\n");
        doc.push_str("```\n\n");
        doc.push_str(&Self::build_options_table(
            Onchain::command(),
            "Onchain-specific Options",
        ));

        doc.push_str("## Examples\n\n");
        doc.push_str("### Manual key splitting\n\n");
        doc.push_str("```bash\n");
        doc.push_str("anchor keysplit manual \\\n");
        doc.push_str("  --keystore-path /path/to/validator_keystore.json \\\n");
        doc.push_str("  --password \"your_keystore_password\" \\\n");
        doc.push_str("  --owner 0x123abc... \\\n");
        doc.push_str("  --operators 1,2,3,4 \\\n");
        doc.push_str("  --output-path /path/to/output.json \\\n");
        doc.push_str("  --nonce 0 \\\n");
        doc.push_str("  --public-keys key1,key2,key3,key4\n");
        doc.push_str("```\n\n");

        doc.push_str("### Onchain key splitting\n\n");
        doc.push_str("```bash\n");
        doc.push_str("anchor keysplit onchain \\\n");
        doc.push_str("  --keystore-path /path/to/validator_keystore.json \\\n");
        doc.push_str("  --password \"your_keystore_password\" \\\n");
        doc.push_str("  --owner 0x123abc... \\\n");
        doc.push_str("  --operators 1,2,3,4 \\\n");
        doc.push_str("  --output-path /path/to/output.json \\\n");
        doc.push_str("  --rpc https://eth-mainnet.provider.com \\\n");
        doc.push_str("  --network mainnet\n");
        doc.push_str("```\n\n");

        doc.push_str("## Output\n\n");
        doc.push_str("These commands will generate a JSON file to be uploaded to the SSV network webapp when ");
        doc.push_str("registering a validator.\n");

        doc
    }

    fn build_options_table(command: Command, section_name: &str) -> String {
        let mut table = String::new();
        table.push_str(&format!("### {}\n\n", section_name));
        table.push_str("| Option | Description | Default |\n");
        table.push_str("| --- | --- | --- |\n");

        for arg in command.get_arguments() {
            if matches!(arg.get_id().as_str(), "help" | "version") {
                continue;
            }

            let id = arg.get_id().as_str();
            let option_name = Self::format_option_name(id, arg);
            let description = arg
                .get_help()
                .map(|h| {
                    h.to_string()
                        .replace("<", "&lt;")
                        .replace(">", "&gt;")
                        .replace("\n", "<br />")
                })
                .unwrap_or_else(|| "No description available".to_string());

            let default = if arg.get_default_values().is_empty() {
                "None".to_string()
            } else {
                let values: Vec<String> = arg
                    .get_default_values()
                    .iter()
                    .map(|v| v.to_string_lossy().to_string())
                    .collect();
                format!("`{}`", values.join(", "))
            };

            table.push_str(&format!(
                "| `{}` | {} | {} |\n",
                option_name, description, default
            ));
        }

        table.push('\n');
        table
    }

    fn format_option_name(id: &str, arg: &clap::Arg) -> String {
        let mut name = format!("--{}", id.replace('_', "-"));

        if arg.get_action().takes_values() {
            let type_hint = match id {
                id if id.contains("address") => "ADDRESS",
                id if id.contains("port") => "PORT",
                id if id.contains("url") || id.contains("nodes") => "URLS",
                id if id.contains("file") || id.contains("path") => "PATH",
                id if id.contains("dir") => "DIR",
                id if id.contains("level") => "LEVEL",
                id if id.contains("size") => "SIZE",
                id if id.contains("number") => "NUMBER",
                id if id.contains("factor") => "FACTOR",
                id if id.contains("origin") => "ORIGIN",
                _ => "VALUE",
            };
            name.push_str(&format!(" <{}>", type_hint));
        }

        name
    }
}

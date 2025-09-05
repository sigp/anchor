use std::{env, fs};

use clap::{Command, CommandFactory};
use keygen::Keygen;
use keysplit::{Manual, Onchain, SharedKeygenOptions};
use logging::FileLoggingFlags;
use tracing::info;

use crate::cli::{
    ExternalApis, HttpApi, MetricsOptions, NetworkOptions, PayloadBuildingOptions, SecurityOptions,
};

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

        doc.push_str(
            r#"# Node Command

The `node` command starts the anchor client as a SSV operator node.

```bash
anchor node [OPTIONS]
```

## Options

"#,
        );

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

        doc.push_str(
            r#"## Examples

```bash
anchor node \
  --network hoodi \
  --datadir /data/anchor \
  --beacon-nodes https://beacon1.example.com,https://beacon2.example.com \
  --execution-rpc https://execution1.example.com,https://execution2.example.com \
  --execution-ws wss://execution1.example.com \
  --listen-addresses 10.0.0.10 \
  --port 9100 \
  --http \
  --http-address 127.0.0.1 \
  --http-port 9200 \
  --unencrypted-http-transport \
  --metrics \
  --metrics-address 127.0.0.1 \
  --metrics-port 9300 \
  --password-file /path/to/your/password
```
"#,
        );

        doc
    }

    /// Generate keygen documentation
    fn generate_keygen_docs() -> String {
        let mut doc = String::new();

        doc.push_str(
            r#"# Keygen Command

The `keygen` command generates RSA keys for SSV operator identification.

```bash
anchor keygen [OPTIONS]
```

"#,
        );

        doc.push_str(&Self::build_options_table(Keygen::command(), "Options"));

        doc.push_str(r#"## Examples

### Basic key generation

This will create an unencrypted `private_key.txt` file containing the newly generated private key and a `public_key.txt` file with the BASE64 encoded public key used for registering the operator.

```bash
anchor keygen
```

### Encrypted key generation

This will create a `encrypted_private_key.json` file encrypted with the provided password and a `public_key.txt` file with the BASE64 encoded public key used for registering the operator. The password must be provided via `--password-file` or interactively when running Anchor.

```bash
anchor keygen --encrypt --output-path /path/to/keys
```"#);

        doc
    }

    /// Generate keysplit documentation
    fn generate_keysplit_docs() -> String {
        let mut doc = String::new();

        doc.push_str(r#"# Keysplit Command

The `keysplit` command is used to split validator keys for distributed validation on the SSV network.

```bash
anchor keysplit <SUBCOMMAND> [OPTIONS]
```

## Subcommands

- `manual` - Split keys with manually provided operator data
- `onchain` - Split keys using operator data from the blockchain

"#);

        doc.push_str(&Self::build_options_table(
            SharedKeygenOptions::command(),
            "Shared Options",
        ));

        doc.push_str(
            r#"## Manual Keysplit Subcommand

```bash
anchor keysplit manual [OPTIONS]
```

"#,
        );

        doc.push_str(&Self::build_options_table(
            Manual::command(),
            "Manual-specific Options",
        ));

        doc.push_str(
            r#"### Example

```bash
anchor keysplit manual \
  --keystore-path /path/to/validator_keystore.json \
  --password "your_keystore_password" \
  --owner 0x123abc... \
  --operators 1,2,3,4 \
  --output-path /path/to/output.json \
  --nonce 0 \
  --public-keys key1,key2,key3,key4
```

"#,
        );

        doc.push_str(
            r#"## Onchain Keysplit Subcommand

```bash
anchor keysplit onchain [OPTIONS]
```

"#,
        );

        doc.push_str(&Self::build_options_table(
            Onchain::command(),
            "Onchain-specific Options",
        ));

        doc.push_str(r#"### Example

```bash
anchor keysplit onchain \
  --keystore-path /path/to/validator_keystore.json \
  --password "your_keystore_password" \
  --owner 0x123abc... \
  --operators 1,2,3,4 \
  --output-path /path/to/output.json \
  --rpc https://eth-mainnet.provider.com \
  --network mainnet
```

## Output

These commands will generate a JSON file to be uploaded to the SSV network webapp when registering a validator.
"#);

        doc
    }

    fn build_options_table(command: Command, section_name: &str) -> String {
        let mut table = format!(
            r#"### {}

| Option | Description | Default |
| --- | --- | --- |
"#,
            section_name
        );

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

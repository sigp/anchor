use clap::{Args, Command};
use clap_markdown::{MarkdownOptions, help_markdown_command_custom};
use global_config::GlobalFlags;

/// Reconstructs the full anchor CLI tree from exported subcommand definitions.
///
/// This avoids depending on the binary crate while producing an identical command tree
/// to the one defined in `anchor/src/main.rs`.
pub fn construct_anchor_cli_tree() -> Command {
    let cmd = Command::new("anchor")
        .about("SSV Validator client. Maintained by Sigma Prime.")
        .subcommand(client::cli())
        .subcommand(keysplit::cli())
        .subcommand(keygen::cli());

    GlobalFlags::augment_args(cmd)
}

/// Generate raw Markdown documentation using clap-markdown.
pub fn generate_markdown(cmd: &Command) -> String {
    let options = MarkdownOptions::new()
        .title("Anchor CLI Reference".to_string())
        .show_footer(false);

    help_markdown_command_custom(cmd, &options)
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};
    use indoc::indoc;

    use super::{construct_anchor_cli_tree, generate_markdown};

    #[derive(Parser, Clone, Debug)]
    pub struct TestFlag {
        #[clap(
            long,
            short = 't',
            value_name = "TEST",
            help = "Test flag.",
            alias = "test"
        )]
        pub test_flag: String,
    }

    #[derive(Parser, Clone, Debug)]
    #[clap(name = "testcli", about = "Test cli interface.", next_line_help = true)]
    struct TestCli {
        #[clap(flatten)]
        pub test_flag: TestFlag,
    }

    #[test]
    fn test_generate_markdown_outputs_correct_document_body() {
        let cmd = TestCli::command();
        let result = generate_markdown(&cmd);

        let expected = indoc! {"
            ## `testcli`

            Test cli interface.

            **Usage:** `testcli --test-flag <TEST>`

            ###### **Options:**

            * `-t`, `--test-flag <TEST>` — Test flag.
        "};

        assert!(
            result.contains(expected),
            "Expected section not found in output.\n\nExpected:\n{expected}\n\nGot:\n{result}"
        );
    }

    #[test]
    fn test_anchor_command_has_expected_subcommands() {
        let cmd = construct_anchor_cli_tree();
        let subcommand_names: Vec<&str> = cmd.get_subcommands().map(|s| s.get_name()).collect();

        assert!(
            subcommand_names.contains(&"node"),
            "Missing 'node' subcommand. Found: {subcommand_names:?}"
        );
        assert!(
            subcommand_names.contains(&"keysplit"),
            "Missing 'keysplit' subcommand. Found: {subcommand_names:?}"
        );
        assert!(
            subcommand_names.contains(&"keygen"),
            "Missing 'keygen' subcommand. Found: {subcommand_names:?}"
        );
    }

    #[test]
    fn test_anchor_command_has_global_flags() {
        let cmd = construct_anchor_cli_tree();
        let arg_ids: Vec<String> = cmd
            .get_arguments()
            .map(|a| a.get_id().to_string())
            .collect();

        assert!(
            arg_ids.contains(&"data_dir".to_string()),
            "Missing 'data_dir' global flag. Found: {arg_ids:?}"
        );
        assert!(
            arg_ids.contains(&"network".to_string()),
            "Missing 'network' global flag. Found: {arg_ids:?}"
        );
        assert!(
            arg_ids.contains(&"debug_level".to_string()),
            "Missing 'debug_level' global flag. Found: {arg_ids:?}"
        );
    }

    #[test]
    fn test_generate_markdown_contains_all_subcommands() {
        let cmd = construct_anchor_cli_tree();
        let result = generate_markdown(&cmd);

        for name in ["node", "keysplit", "keygen"] {
            assert!(
                result.contains(&format!("`anchor {name}`")),
                "Generated markdown missing documentation for '{name}' subcommand"
            );
        }
    }
}

use clap::{Command, builder::StyledStr};

use crate::errors::DocGenError;

pub(crate) fn render_help_string(command: &mut Command) -> String {
    wrap_help_as_code_block(&command.render_long_help())
}

pub(crate) fn render_subcommand_help_snippet(
    command: &mut Command,
    name: &str,
    cli_tree: &str,
) -> Result<String, DocGenError> {
    let subcmd =
        command
            .find_subcommand_mut(name)
            .ok_or_else(|| DocGenError::SubcommandNotFound {
                subcommand: name.to_string(),
                cli_tree: cli_tree.to_string(),
            })?;
    Ok(render_help_string(subcmd))
}

/// Helper function that converts a word to title-case format.
pub(crate) fn to_title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Wrap clap `render_long_help()` output in a fenced code block for MDX embedding.
fn wrap_help_as_code_block(help: &StyledStr) -> String {
    format!("```text\n{help}\n```\n")
}

#[cfg(test)]
mod tests {
    use clap::Command;

    use super::{
        render_help_string, render_subcommand_help_snippet, to_title_case, wrap_help_as_code_block,
    };
    use crate::{anchor_command, errors::DocGenError};

    #[test]
    fn test_wrap_help_as_code_block_produces_fenced_block() {
        let mut cmd = Command::new("test")
            .about("A test command")
            .arg(clap::Arg::new("option").long("option").help("An option"));
        let help = cmd.render_long_help();
        let wrapped = wrap_help_as_code_block(&help);

        assert!(wrapped.starts_with("```text\n"));
        assert!(wrapped.ends_with("\n```\n"));
        assert!(wrapped.contains("--option"));
        assert!(wrapped.contains("A test command"));
    }

    #[test]
    fn test_to_title_case_capitalizes_first_letter() {
        assert_eq!(to_title_case("anchor"), "Anchor");
        assert_eq!(to_title_case("Anchor"), "Anchor");
    }

    #[test]
    fn test_render_subcommand_help_snippet_returns_error_for_missing_subcommand() {
        let mut cmd = anchor_command();
        cmd.build();

        let result = render_subcommand_help_snippet(&mut cmd, "nonexistent", "anchor");

        assert!(
            matches!(result, Err(DocGenError::SubcommandNotFound { .. })),
            "Expected SubcommandNotFound, got: {result:?}"
        );
    }

    #[test]
    fn test_render_help_string_contains_all_visible_node_args() {
        let mut cmd = anchor_command();
        cmd.build();

        let node = cmd.find_subcommand("node").unwrap();
        let expected_args: Vec<_> = node
            .get_arguments()
            .filter(|a| !a.is_positional() && !a.is_hide_set())
            .filter_map(|a| a.get_long())
            .map(|l| l.to_string())
            .collect();

        let result = render_help_string(cmd.find_subcommand_mut("node").unwrap());

        for long in &expected_args {
            assert!(
                result.contains(&format!("--{long}")),
                "Arg '--{long}' missing from rendered node help"
            );
        }
    }

    #[test]
    fn test_render_help_string_contains_node_help_headings() {
        let mut cmd = anchor_command();
        cmd.build();

        let result = render_help_string(cmd.find_subcommand_mut("node").unwrap());

        for heading in [
            "Security Options",
            "External APIs",
            "HTTP API Options",
            "Network Options",
            "Metrics Options",
            "Payload Building Options",
            "Logging Options",
        ] {
            assert!(
                result.contains(heading),
                "Missing heading '{heading}' in rendered node help"
            );
        }
    }

    #[test]
    fn test_render_subcommand_help_snippet_keysplit_contains_subcommands() {
        let mut cmd = anchor_command();
        cmd.build();

        let result = render_subcommand_help_snippet(&mut cmd, "keysplit", "anchor").unwrap();

        assert!(
            result.contains("onchain"),
            "Missing 'onchain' subcommand in keysplit help"
        );
        assert!(
            result.contains("manual"),
            "Missing 'manual' subcommand in keysplit help"
        );
    }
}

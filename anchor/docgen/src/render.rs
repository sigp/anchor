use clap::{Command, builder::StyledStr};

use crate::errors::DocGenError;

/// Target rendered line width minus some arbitrary amount.
/// Clap wraps at this narrower width to leave room for a small indent boost.
const WRAP_WIDTH: usize = 74;
/// Extra spaces added per indent level so Clap's 2-space hierarchy reads clearly in the docs.
const INDENT_BOOST: usize = 2;

/// Renders a long help description of the `Command` provided to the function with a fixed terminal
/// width.
///
/// This is required for correctly indented and formatted code blocks when help snippets are written
/// to project documentation. Text is wrapped with specified width so that after indentation is
/// increased, the output fits within WRAP_WIDTH + INDENT_BOOST. next_line_help(false) keeps flag
/// name and description on the same line.
fn render_wrapped_help(command: &Command) -> StyledStr {
    command
        .clone()
        .term_width(WRAP_WIDTH)
        .next_line_help(false)
        .render_long_help()
}

/// Render a command's own help as a fenced code block (no child subcommands).
pub(crate) fn render_help_flat(command: &Command) -> String {
    let help_string = render_wrapped_help(command).to_string();
    wrap_help_as_code_block(&help_string)
}

/// Render a command's help followed by each visible subcommand's help, all in one fenced block.
fn render_help_with_subcommands(command: &Command) -> String {
    let mut help_string = render_wrapped_help(command).to_string();

    for subcommand in command.get_subcommands() {
        let subcommand_name = subcommand.get_name();
        let help_body = render_wrapped_help(subcommand).to_string();
        help_string.push_str(&format!("\n{subcommand_name}:\n\n"));
        help_string.push_str(&help_body);
    }
    wrap_help_as_code_block(&help_string)
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
    Ok(render_help_with_subcommands(subcmd))
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
///
/// Clap already word-wraps at the configured `term_width`. This function boosts
/// indentation proportionally so Clap's tight 2-space hierarchy reads clearly.
fn wrap_help_as_code_block(help: &str) -> String {
    let mut out = String::from("```text\n");
    for line in help.lines() {
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent > 0 {
            let boosted = indent + (indent / 2).max(INDENT_BOOST);
            out.extend(std::iter::repeat_n(' ', boosted));
            out.push_str(line.trim_start_matches(' '));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out.push_str("```\n");
    out
}

#[cfg(test)]
mod tests {
    use clap::Command;

    use super::{
        render_help_flat, render_help_with_subcommands, render_subcommand_help_snippet,
        to_title_case, wrap_help_as_code_block,
    };
    use crate::{anchor_command, errors::DocGenError};

    #[test]
    fn test_wrap_help_as_code_block_produces_fenced_block() {
        let mut cmd = Command::new("test")
            .about("A test command")
            .arg(clap::Arg::new("option").long("option").help("An option"));
        let help = cmd.render_long_help().to_string();
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
    fn test_render_help_with_subcommands_contains_all_visible_node_args() {
        let mut cmd = anchor_command();
        cmd.build();

        let node = cmd.find_subcommand("node").unwrap();
        let expected_args: Vec<_> = node
            .get_arguments()
            .filter(|a| !a.is_positional() && !a.is_hide_set())
            .filter_map(|a| a.get_long())
            .map(|l| l.to_string())
            .collect();

        let result = render_help_with_subcommands(cmd.find_subcommand("node").unwrap());

        for long in &expected_args {
            assert!(
                result.contains(&format!("--{long}")),
                "Arg '--{long}' missing from rendered node help"
            );
        }
    }

    #[test]
    fn test_render_help_with_subcommands_contains_node_help_headings() {
        let mut cmd = anchor_command();
        cmd.build();

        let result = render_help_with_subcommands(cmd.find_subcommand("node").unwrap());

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

    #[test]
    fn test_render_help_flat_excludes_subcommand_content() {
        let mut cmd = anchor_command();
        cmd.build();

        let result = render_help_flat(cmd.find_subcommand("keysplit").unwrap());

        assert!(
            !result.contains("onchain:"),
            "Flat render should not contain nested subcommand 'onchain'"
        );
        assert!(
            !result.contains("manual:"),
            "Flat render should not contain nested subcommand 'manual'"
        );
    }

    #[test]
    fn test_render_help_flat_produces_fenced_block() {
        let mut cmd = anchor_command();
        cmd.build();

        let result = render_help_flat(cmd.find_subcommand("node").unwrap());

        assert!(result.starts_with("```text\n"));
        assert!(result.ends_with("\n```\n"));
    }

    #[test]
    fn test_render_help_with_subcommands_produces_fenced_block() {
        let mut cmd = anchor_command();
        cmd.build();

        let result = render_help_with_subcommands(cmd.find_subcommand("keysplit").unwrap());

        assert!(result.starts_with("```text\n"));
        assert!(result.ends_with("\n```\n"));
    }
}

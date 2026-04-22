use std::fmt::Write;

use clap::{Arg, Command};

use crate::{
    errors::DocGenError,
    format::{format_default, format_description, format_option, group_args_by_clap_groups},
};

/// A postprocessing function that creates styled `.mdx` markdown tables from grouped CLI arguments.
///
/// The output is formatted to match the style
/// ```markdown
/// | Option | Description | Default |
/// | --- | --- | --- |
/// | --option | Option description (possible values: ...) | `default` |
/// ```
fn generate_formatted_option_table_doc(
    groups: &[(Option<String>, Vec<&Arg>)],
    heading_prefix: &str,
) -> Result<String, DocGenError> {
    let mut output = String::new();

    for (heading, group_args) in groups {
        if let Some(heading_text) = heading {
            writeln!(output, "{heading_prefix} {heading_text}\n").map_err(|e| {
                DocGenError::RenderOptionGroup {
                    group: heading_text.to_string(),
                    source: e,
                }
            })?;
        }
        let group_name = heading.as_deref().unwrap_or("Ungrouped").to_string();
        writeln!(output, "| Option | Description | Default |").map_err(|e| {
            DocGenError::RenderOptionGroup {
                group: group_name.clone(),
                source: e,
            }
        })?;
        writeln!(output, "| --- | --- | --- |").map_err(|e| DocGenError::RenderOptionGroup {
            group: group_name.clone(),
            source: e,
        })?;
        for arg in group_args {
            write_arg_table_row(&mut output, arg)?;
        }
        writeln!(output).map_err(|e| DocGenError::RenderOptionGroup {
            group: group_name,
            source: e,
        })?;
    }
    Ok(output)
}

/// Render a command's options as markdown tables grouped by struct-derived ArgGroups.
pub fn render_options_tables(cmd: &Command, heading_prefix: &str) -> Result<String, DocGenError> {
    let args: Vec<_> = cmd
        .get_arguments()
        .filter(|a| !a.is_positional() && !a.is_hide_set())
        .collect();

    let mut groups = group_args_by_clap_groups(cmd, &args);
    if groups.len() == 1 {
        groups[0].0 = None;
    }
    generate_formatted_option_table_doc(&groups, heading_prefix)
}

/// Write a single argument row: `| Option | Description | Default |`
fn write_arg_table_row(output: &mut String, arg: &Arg) -> Result<(), DocGenError> {
    let option_str = format_option(arg);
    let description = format_description(arg)?;
    let default = format_default(arg);

    writeln!(output, "| {option_str} | {description} | {default} |").map_err(|e| {
        DocGenError::RenderOptionGroup {
            group: arg.get_help_heading().unwrap_or("Ungrouped").to_string(),
            source: e,
        }
    })
}

/// Generate the CLI reference snippet for global options.
pub fn generate_cli_reference_snippet(cmd: &Command) -> Result<String, DocGenError> {
    render_options_tables(cmd, "####")
}

/// Generate CLI help snippet for a command with nested subcommands.
fn generate_nested_command_reference_snippet(cmd: &Command) -> Result<String, DocGenError> {
    let mut output = String::new();
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() {
            continue;
        }
        let name = sub.get_name();
        let about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();

        writeln!(output, "#### {name} Subcommand\n").map_err(|e| {
            DocGenError::RenderOptionGroup {
                group: name.to_string(),
                source: e,
            }
        })?;
        writeln!(output, "{about}\n").map_err(|e| DocGenError::RenderOptionGroup {
            group: name.to_string(),
            source: e,
        })?;
        output.push_str(&render_options_tables(sub, "#####")?);
    }
    Ok(output)
}

/// Generate the CLI reference snippet for a subcommand page.
pub fn generate_subcommand_reference_snippet(
    cmd: &Command,
    subcommand_name: &str,
) -> Result<String, DocGenError> {
    let subcmd =
        cmd.find_subcommand(subcommand_name)
            .ok_or_else(|| DocGenError::SubcommandNotFound {
                subcommand: subcommand_name.to_string(),
                cli_tree: cmd.get_name().to_string(),
            })?;

    if subcmd.get_subcommands().any(|s| !s.is_hide_set()) {
        generate_nested_command_reference_snippet(subcmd)
    } else {
        generate_cli_reference_snippet(subcmd)
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;
    use crate::anchor_command;

    #[derive(Parser, Clone, Debug)]
    pub struct TestFlag {
        #[clap(
            long,
            short = 't',
            value_name = "TEST",
            help = "Test flag.",
            alias = "test",
            help_heading = "Test Group"
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
    fn test_generate_formatted_option_table_doc_returns_valid_help_string() {
        let cmd = TestCli::command();
        let arg = cmd.get_arguments().next().unwrap();
        let groups = Vec::from([(Some("Test Group".to_string()), vec![arg])]);
        let result = generate_formatted_option_table_doc(&groups, "####").unwrap();

        assert!(result.contains("| Option | Description | Default |"));
        assert!(result.contains("#### Test Group"));
        assert!(result.contains("`-t`, `--test-flag <TEST>`"));
        assert!(result.contains("Test flag."));
    }

    #[test]
    fn test_render_options_tables_produces_table() {
        let cmd = TestCli::command();
        let result = render_options_tables(&cmd, "####").unwrap();

        assert!(result.contains("| Option | Description | Default |"));
        assert!(result.contains("`-t`, `--test-flag <TEST>`"));
    }

    #[test]
    fn test_render_node_options_has_headings() {
        let cmd = anchor_command();
        let result = generate_subcommand_reference_snippet(&cmd, "node").unwrap();

        for heading in [
            "Security Options",
            "External APIs",
            "HTTP API",
            "Network Options",
            "Metrics Options",
            "Payload Building Options",
            "Logging Options",
            "Additional Options",
        ] {
            assert!(
                result.contains(heading),
                "Missing heading '{heading}' in node options:\n{result}"
            );
        }
    }

    #[test]
    fn test_render_keysplit_has_subcommand_sections() {
        let cmd = anchor_command();
        let result = generate_subcommand_reference_snippet(&cmd, "keysplit").unwrap();

        assert!(
            result.contains("#### onchain Subcommand"),
            "Missing onchain subcommand section:\n{result}"
        );
        assert!(
            result.contains("#### manual Subcommand"),
            "Missing manual subcommand section:\n{result}"
        );
    }

    #[test]
    fn test_render_cli_snippet_contains_options_table() {
        let cmd = anchor_command();
        let result = generate_cli_reference_snippet(&cmd).unwrap();

        assert!(result.contains("| Option | Description | Default |"));
    }

    #[test]
    fn test_node_groups_cover_all_visible_args() {
        let cmd = anchor_command();
        let node = cmd.find_subcommand("node").unwrap();
        let result = generate_subcommand_reference_snippet(&cmd, "node").unwrap();

        for arg in node.get_arguments() {
            if !arg.is_positional() && !arg.is_hide_set() {
                let long = arg.get_long().unwrap();
                assert!(
                    result.contains(&format!("--{long}")),
                    "Arg '--{long}' missing from generated node docs"
                );
            }
        }
    }
}

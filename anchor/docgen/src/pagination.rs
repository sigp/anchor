use std::fmt::Write;

use clap::{Arg, ArgAction, Command};

use crate::errors::DocGenError;

/// Sentinel markers for generated CLI reference sections in .mdx files.
pub const CLI_REFERENCE_START: &str = "<!-- CLI_REFERENCE_START -->";
pub const CLI_REFERENCE_END: &str = "<!-- CLI_REFERENCE_END -->";

/// Groups arguments by common help_heading values.
///
/// Returns a vector of tuples containing the group name and the arguments that belong to that
/// group, preserving definition order.
fn group_args_by_help_heading<'a>(args: &[&'a Arg]) -> Vec<(Option<&'a str>, Vec<&'a Arg>)> {
    let mut groups: Vec<(Option<&'a str>, Vec<&'a Arg>)> = Vec::new();
    for arg in args {
        let heading = arg.get_help_heading();
        if let Some(group) = groups.iter_mut().find(|(h, _)| *h == heading) {
            group.1.push(arg);
        } else {
            groups.push((heading, vec![arg]));
        }
    }
    groups
}

/// Create structured markdown table documentation from groupings of CLI arguments by a semantic
/// heading.
///
/// A components are rendered in the order: heading, option table header (column names), option
/// table content (created by row). A clap::Arg is rendered per row per group.
fn generate_formatted_option_table_doc(
    groups: &[(Option<&str>, Vec<&Arg>)],
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
        writeln!(output, "| Option | Description | Default |").map_err(|e| {
            DocGenError::RenderOptionGroup {
                group: heading.unwrap_or("Ungrouped").to_string(),
                source: e,
            }
        })?;
        writeln!(output, "| --- | --- | --- |").map_err(|e| DocGenError::RenderOptionGroup {
            group: heading.unwrap_or("Ungrouped").to_string(),
            source: e,
        })?;
        for arg in group_args {
            write_arg_table_row(&mut output, arg)?;
        }
        writeln!(output).map_err(|e| DocGenError::RenderOptionGroup {
            group: heading.unwrap_or("Ungrouped").to_string(),
            source: e,
        })?;
    }
    Ok(output)
}

/// Render a clap CLI command's options as markdown tables grouped by the `help_heading` option.
///
/// Arguments are grouped by their `help_heading` attribute, preserving definition order.
/// Each group is rendered as a separate table with the heading at `heading_prefix` level.
pub fn render_options_tables(cmd: &Command, heading_prefix: &str) -> Result<String, DocGenError> {
    // Named and visible arguments only.
    let args: Vec<_> = cmd
        .get_arguments()
        .filter(|a| !a.is_positional() && !a.is_hide_set())
        .collect();

    let groups = group_args_by_help_heading(&args);
    generate_formatted_option_table_doc(&groups, heading_prefix)
}

/// Handles writing each row to the option table and formatting the option, description, and default
/// value cells.
///
/// The structure is -> | Option | Description | Default |
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

/// Outputs a String with the long and/or short name(s) of the clap::Arg along with its value name
/// if it takes a value.
///
/// Checks whether the argument has a short and/or long name and formats and handles possible
/// permutations of those. This is a utility function to help format the "Option" column of the
/// options table.
fn format_option(arg: &Arg) -> String {
    let value_name = arg
        .get_value_names()
        .and_then(|names| names.first())
        .map(|n| n.to_string())
        .unwrap_or_else(|| arg.get_id().to_string().to_ascii_uppercase());

    match (arg.get_short(), arg.get_long()) {
        (Some(short), Some(long)) => {
            if arg.get_action().takes_values() {
                format!("`-{short}`, `--{long} <{value_name}>`")
            } else {
                format!("`-{short}`, `--{long}`")
            }
        }
        (None, Some(long)) => {
            if arg.get_action().takes_values() {
                format!("`--{long} <{value_name}>`")
            } else {
                format!("`--{long}`")
            }
        }
        (Some(short), None) => {
            if arg.get_action().takes_values() {
                format!("`-{short} <{value_name}>`")
            } else {
                format!("`-{short}`")
            }
        }
        (None, None) => format!("`<{value_name}>`"),
    }
}

/// Format the description for a clap::Arg, including its help text, possible values, and default
/// value(s).
///
/// The description is determined by the `help` text of the argument. Possible values are shown as a
/// list of options in the help string if they are present in the cli tree definition.
fn format_description(arg: &Arg) -> Result<String, DocGenError> {
    let mut desc = arg.get_help().map(|h| h.to_string()).unwrap_or_default();

    // Collapse newlines and excess whitespace for table cell.
    desc = desc.replace('\n', " ");
    while desc.contains("  ") {
        desc = desc.replace("  ", " ");
    }

    // Escape pipe characters for markdown table.
    desc = desc.replace('|', "\\|");

    // Append possible values if present.
    let possible_values: Vec<_> = arg
        .get_possible_values()
        .into_iter()
        .filter(|pv| !pv.is_hide_set())
        .collect();

    // Collect possible values into a comma-separated list of options.
    if !possible_values.is_empty() && !matches!(arg.get_action(), ArgAction::SetTrue) {
        let values_str: String = possible_values
            .iter()
            .map(|pv| pv.get_name().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        if !desc.is_empty() {
            desc.push(' ');
        }
        write!(desc, "(possible values: {values_str})").map_err(|e| {
            DocGenError::RenderOptionGroup {
                group: arg.get_help_heading().unwrap_or("Ungrouped").to_string(),
                source: e,
            }
        })?;
    }

    Ok(desc)
}

/// Format the default value(s) for a clap::Arg.
///
/// Returns a comma-separated list of default values if they are present in the cli tree definition.
/// If no default value is present, returns an empty string.
fn format_default(arg: &Arg) -> String {
    let defaults = arg.get_default_values();
    if defaults.is_empty() {
        return String::new();
    }

    defaults
        .iter()
        .map(|v| format!("`{}`", v.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Generate the CLI reference content for `cli.mdx` (global options).
pub fn generate_cli_page_content(cmd: &Command) -> Result<String, DocGenError> {
    let mut output = String::new();
    writeln!(output, "### Global Options\n").map_err(|e| DocGenError::RenderOptionGroup {
        group: "Global Options".to_string(),
        source: e,
    })?;
    output.push_str(&render_options_tables(cmd, "####")?);
    Ok(output)
}

/// Generate CLI help content for a command without producing detailed subcommand information.
fn generate_flat_command_page_content(cmd: &Command) -> Result<String, DocGenError> {
    let mut output = String::new();
    writeln!(output, "### Options\n").map_err(|e| DocGenError::RenderOptionGroup {
        group: "Options".to_string(),
        source: e,
    })?;
    output.push_str(&render_options_tables(cmd, "####")?);
    Ok(output)
}

/// Generate CLI help content for a command with detailed nested subcommand information.
///
/// Each subcommand information is rendered in its own section.
fn generate_nested_command_page_content(cmd: &Command) -> Result<String, DocGenError> {
    let mut output = String::new();
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() {
            continue;
        }
        let name = sub.get_name();
        let about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
        let usage = sub
            .clone()
            .render_usage()
            .to_string()
            .replace("Usage: ", "");

        writeln!(output, "### {name} Subcommand\n").map_err(|e| {
            DocGenError::RenderOptionGroup {
                group: name.to_string(),
                source: e,
            }
        })?;
        writeln!(output, "{about}\n").map_err(|e| DocGenError::RenderOptionGroup {
            group: name.to_string(),
            source: e,
        })?;
        writeln!(output, "```bash\nanchor {name} {usage}\n```\n").map_err(|e| {
            DocGenError::RenderOptionGroup {
                group: name.to_string(),
                source: e,
            }
        })?;
        output.push_str(&render_options_tables(sub, "####")?);
    }
    Ok(output)
}

/// Generate the CLI reference content for a subcommand page.
///
/// For commands with nested subcommands, each subcommand is rendered with its own heading, usage,
/// and options table. Options tables are rendered as a single group for flat commands.
pub fn generate_subcommand_page_content(
    cmd: &Command,
    subcommand_name: &str,
) -> Result<String, DocGenError> {
    let subcmd =
        cmd.find_subcommand(subcommand_name)
            .ok_or_else(|| DocGenError::SubcommandNotFound {
                subcommand: subcommand_name.to_string(),
                cli_tree: cmd.get_name().to_string(),
            })?;

    let has_subcommands = subcmd.get_subcommands().any(|s| !s.is_hide_set());

    if !has_subcommands {
        // Flat command — render "### Options" with grouped tables.
        generate_flat_command_page_content(subcmd)
    } else {
        // Command with subcommands — render per-subcommand sections.
        generate_nested_command_page_content(subcmd)
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;
    use crate::construct_anchor_cli_tree;

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
    fn test_group_args_by_help_heading_outputs_correct_groupings() {
        let cmd = TestCli::command();
        let args: Vec<_> = cmd.get_arguments().collect();
        let result = group_args_by_help_heading(&args);

        assert_eq!(
            result[0].0,
            Some("Test Group"),
            "Expected group heading 'Test Group' in group_args_by_help_heading function.",
        );
        assert_eq!(
            result[0].1[0],
            cmd.get_arguments().next().unwrap(),
            "Incorrect argument grouping in group_args_by_help_heading function."
        );
    }

    #[test]
    fn test_generate_formatted_option_table_doc_returns_valid_help_string() {
        let cmd = TestCli::command();
        let arg = cmd.get_arguments().next().unwrap();
        let groups = Vec::from([(Some("Test Group"), vec![arg])]);
        let result = generate_formatted_option_table_doc(&groups, "####").unwrap();

        assert!(
            result.contains("| Option | Description | Default |"),
            "Missing table header in output:\n{result}"
        );
        assert!(
            result.contains("#### Test Group"),
            "Missing group heading in output:\n{result}"
        );
        assert!(
            result.contains("`-t`, `--test-flag <TEST>`"),
            "Missing option in table:\n{result}"
        );
        assert!(
            result.contains("Test flag."),
            "Missing description in table:\n{result}"
        );
    }

    #[test]
    fn test_render_options_tables_produces_table() {
        let cmd = TestCli::command();
        let result = render_options_tables(&cmd, "####").unwrap();

        assert!(
            result.contains("| Option | Description | Default |"),
            "Missing table header in output:\n{result}"
        );
        assert!(
            result.contains("`-t`, `--test-flag <TEST>`"),
            "Missing option in table:\n{result}"
        );
        assert!(
            result.contains("Test flag."),
            "Missing description in table:\n{result}"
        );
    }

    #[test]
    fn test_render_node_options_has_headings() {
        let cmd = construct_anchor_cli_tree();
        let result = generate_subcommand_page_content(&cmd, "node").unwrap();

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
        let cmd = construct_anchor_cli_tree();
        let result = generate_subcommand_page_content(&cmd, "keysplit").unwrap();

        assert!(
            result.contains("### onchain Subcommand"),
            "Missing onchain subcommand section:\n{result}"
        );
        assert!(
            result.contains("### manual Subcommand"),
            "Missing manual subcommand section:\n{result}"
        );
    }
}

use std::fmt::Write;

use clap::{Arg, ArgAction, Command};

use crate::errors::DocGenError;

/// Sentinel markers for generated CLI reference sections in .mdx files.
/// MDX uses JSX-style comments (`{/* */}`) rather than HTML comments (`<!-- -->`).
pub const CLI_REFERENCE_START: &str = "{/* CLI_REFERENCE_START */}";
pub const CLI_REFERENCE_END: &str = "{/* CLI_REFERENCE_END */}";

/// Groups arguments by their `help_heading` value, preserving definition order.
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

/// Create markdown tables from grouped CLI arguments.
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

/// Render a command's options as markdown tables grouped by `help_heading`.
pub fn render_options_tables(cmd: &Command, heading_prefix: &str) -> Result<String, DocGenError> {
    let args: Vec<_> = cmd
        .get_arguments()
        .filter(|a| !a.is_positional() && !a.is_hide_set())
        .collect();

    let groups = group_args_by_help_heading(&args);
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

/// Format the option name column (short/long flags and value name).
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

/// Format the description column, including possible values.
fn format_description(arg: &Arg) -> Result<String, DocGenError> {
    let mut desc = arg.get_help().map(|h| h.to_string()).unwrap_or_default();

    // Collapse newlines and excess whitespace for table cell.
    desc = desc.replace('\n', " ");
    while desc.contains("  ") {
        desc = desc.replace("  ", " ");
    }

    // Escape pipe characters for markdown table.
    desc = desc.replace('|', "\\|");

    // Escape curly braces for MDX (unescaped braces are interpreted as JSX expressions).
    desc = desc.replace('{', "\\{").replace('}', "\\}");

    // Append possible values if present.
    let possible_values: Vec<_> = arg
        .get_possible_values()
        .into_iter()
        .filter(|pv| !pv.is_hide_set())
        .collect();

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

/// Format the default value column.
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

/// Generate CLI help content for a flat command (no subcommands).
fn generate_flat_command_page_content(cmd: &Command) -> Result<String, DocGenError> {
    let mut output = String::new();
    writeln!(output, "### Options\n").map_err(|e| DocGenError::RenderOptionGroup {
        group: "Options".to_string(),
        source: e,
    })?;
    output.push_str(&render_options_tables(cmd, "####")?);
    Ok(output)
}

/// Generate CLI help content for a command with nested subcommands.
fn generate_nested_command_page_content(cmd: &Command) -> Result<String, DocGenError> {
    let parent_name = cmd.get_name();
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
        writeln!(output, "```bash\nanchor {parent_name} {usage}\n```\n").map_err(|e| {
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
        generate_flat_command_page_content(subcmd)
    } else {
        generate_nested_command_page_content(subcmd)
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
    fn test_group_args_by_help_heading_outputs_correct_groupings() {
        let cmd = TestCli::command();
        let args: Vec<_> = cmd.get_arguments().collect();
        let result = group_args_by_help_heading(&args);

        assert_eq!(result[0].0, Some("Test Group"));
        assert_eq!(result[0].1[0], cmd.get_arguments().next().unwrap());
    }

    #[test]
    fn test_generate_formatted_option_table_doc_returns_valid_help_string() {
        let cmd = TestCli::command();
        let arg = cmd.get_arguments().next().unwrap();
        let groups = Vec::from([(Some("Test Group"), vec![arg])]);
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
        let cmd = anchor_command();
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

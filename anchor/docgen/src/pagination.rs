use std::{collections::HashSet, fmt::Write};

use clap::{Arg, ArgAction, Command};

use crate::errors::DocGenError;

/// Sentinel markers for generated CLI reference sections in .mdx files.
/// MDX uses JSX-style comments (`{/* */}`) rather than HTML comments (`<!-- -->`).
pub const CLI_REFERENCE_START: &str = "{/* CLI_REFERENCE_START */}";
pub const CLI_REFERENCE_END: &str = "{/* CLI_REFERENCE_END */}";

/// Convert a clap ArgGroup ID (PascalCase struct name) to a human-readable heading.
///
/// Hard-coded display names feature for certain groups for styling preferences.
/// For example -> "...Apis" to "... APIs".
fn group_display_name(group_id: &str) -> String {
    match group_id {
        "ExternalApis" => "External APIs".to_string(),
        "HttpApiOptions" => "HTTP API".to_string(),
        "FileLoggingFlags" => "Logging Options".to_string(),
        _ => split_pascal_case(group_id),
    }
}

/// Split a PascalCase identifier into space-separated words.
/// e.g. "SecurityOptions" → "Security Options"
fn split_pascal_case(s: &str) -> String {
    let mut words = Vec::new();
    let mut current = String::new();
    for ch in s.chars() {
        if ch.is_uppercase() && !current.is_empty() {
            words.push(current);
            current = String::new();
        }
        current.push(ch);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words.join(" ")
}

/// Group arguments by the ArgGroups registered on the command.
///
/// clap_derive automatically creates an ArgGroup for each `#[derive(Args)]` struct,
/// with the struct's kebab-cased name as the group ID and the struct's direct args
/// as members.
fn group_args_by_clap_groups<'a>(
    cmd: &Command,
    args: &[&'a Arg],
) -> Vec<(Option<String>, Vec<&'a Arg>)> {
    // Collect groups that have members (sub-structs with direct args, not parent
    // structs that contain flatten fields — those get empty groups per clap's design).
    let groups: Vec<_> = cmd
        .get_groups()
        .filter(|g| g.get_args().next().is_some())
        .collect();

    let mut result: Vec<(Option<String>, Vec<&'a Arg>)> = Vec::new();
    let mut assigned: HashSet<&str> = HashSet::new();

    for group in &groups {
        // Collects IDs of command args in a group.
        let group_arg_ids: HashSet<_> = group.get_args().map(|id| id.as_str()).collect();

        // Finds command args that belong to this group.
        let matching: Vec<_> = args
            .iter()
            .filter(|a| group_arg_ids.contains(a.get_id().as_str()))
            .copied()
            .collect();

        // Tracks assigned args and adds matching group and args to result.
        if !matching.is_empty() {
            let name = group_display_name(group.get_id().as_str());
            for a in &matching {
                assigned.insert(a.get_id().as_str());
            }
            result.push((Some(name), matching));
        }
    }

    // Remaining args not in any other group are added at the end under "Additional Options".
    let remaining: Vec<_> = args
        .iter()
        .filter(|a| !assigned.contains(a.get_id().as_str()))
        .copied()
        .collect();
    if !remaining.is_empty() {
        result.push((Some("Additional Options".to_string()), remaining));
    }

    result
}

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

    let groups = group_args_by_clap_groups(cmd, &args);
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
///
/// Short and long flags are checked and formatted as part of the return value.
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
///
/// This uses the `help` string and appends possible values if they exist. The string is
/// formatted to be suitable for markdown table cells.
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

    // Append possible values inferred from the clap struct if present.
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
        return "None".to_string();
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
    output.push_str(&render_options_tables(cmd, "###")?);
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
///
/// Builds up a string with doc sections for each subcommand in `cmd`.
/// Each subcommand name, about message, and help description are rendered.
fn generate_nested_command_page_content(cmd: &Command) -> Result<String, DocGenError> {
    let mut output = String::new();
    for sub in cmd.get_subcommands() {
        // Skip hidden subcommands.
        if sub.is_hide_set() {
            continue;
        }
        let name = sub.get_name();
        let about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();

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
    fn test_group_args_by_clap_groups_outputs_correct_groupings() {
        let cmd = TestCli::command();
        let args: Vec<_> = cmd.get_arguments().collect();
        let result = group_args_by_clap_groups(&cmd, &args);

        // The TestFlag struct creates a "TestFlag" ArgGroup with its arg.
        assert!(!result.is_empty());
        // The group should contain the test_flag arg with display name "Test Flag".
        let test_flag_group = result
            .iter()
            .find(|(name, _)| name.as_deref() == Some("Test Flag"));
        assert!(
            test_flag_group.is_some(),
            "Expected a 'Test Flag' group, got: {:?}",
            result.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );
        let (_, group_args) = test_flag_group.unwrap();
        assert!(
            group_args
                .iter()
                .any(|a| a.get_id().as_str() == "test_flag")
        );
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

    #[test]
    fn test_node_groups_cover_all_visible_args() {
        let cmd = anchor_command();
        let node = cmd.find_subcommand("node").unwrap();
        let result = generate_subcommand_page_content(&cmd, "node").unwrap();

        // Every visible non-positional arg should appear in the output.
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

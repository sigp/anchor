//! Contains documentation generator help string formatting to produce markdown content for CLI
//! reference pages.
use std::{collections::HashSet, fmt::Write};

use clap::{Arg, ArgAction, Command};

use crate::errors::DocGenError;


/// A container for a group of CLI arguments that belong to a common semantic group.
type CliArgGrouping<'a> = Vec<&'a Arg>;

/// A container of CLI argument semantic groupings with optional group display names.
type GroupedCliArgs<'a> = Vec<(Option<String>, CliArgGrouping<'a>)>;

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
/// In addition, handles consecutive uppercase letters as a single word, e.g. "HTTPApi" → "HTTP
/// Api".
fn split_pascal_case(s: &str) -> String {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous = '\0';
    for ch in s.chars() {
        if ch.is_uppercase() && !current.is_empty() && !previous.is_uppercase() {
            words.push(current);
            current = String::new();
        }
        // Handle acronym followed by regular word, e.g. "HTTPApi" → "HTTP Api"
        // In the case of a run of uppercase letters, the last uppercase detected is treated as the
        // start of the next word.
        if !ch.is_uppercase() && current.len() > 1 && current.chars().all(|c| c.is_uppercase()) {
            let last_char = current.pop().unwrap();
            words.push(current);
            current = String::new();
            current.push(last_char);
        }
        current.push(ch);
        previous = ch;
    }
    if !current.is_empty() {
        words.push(current);
    }
    words.join(" ")
}

/// Group arguments by the ArgGroups registered on the command.
///
/// clap_derive automatically creates an ArgGroup for each `#[derive(Parser)]` struct,
/// with the struct's kebab-cased name as the group ID and the struct's direct args
/// as members.
pub(crate) fn group_args_by_clap_groups<'a>(
    cmd: &Command,
    args: &[&'a Arg],
) -> GroupedCliArgs<'a> {
    // Collect groups that have members (sub-structs with direct args, not parent
    // structs that contain flatten fields — those get empty groups per clap's design).
    let groups: Vec<_> = cmd
        .get_groups()
        .filter(|g| g.get_args().next().is_some())
        .collect();

    let mut result: GroupedCliArgs<'a> = Vec::new();
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

/// Format the option name column (short/long flags and value name).
///
/// Short and long flags are checked and formatted as part of the return value.
pub(crate) fn format_option(arg: &Arg) -> String {
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
pub(crate) fn format_description(arg: &Arg) -> Result<String, DocGenError> {
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
pub(crate) fn format_default(arg: &Arg) -> String {
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

#[cfg(test)]
mod tests {
    use clap::{Arg, ArgAction, CommandFactory, Parser, ValueEnum, builder::EnumValueParser};

    use super::{format_description, format_option, group_args_by_clap_groups};

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
    fn test_split_pascal_case_splits_acronym_cases() {
        assert_eq!(
            super::split_pascal_case("HTTPApiOptions"),
            "HTTP Api Options"
        );
        assert_eq!(super::split_pascal_case("ExternalApis"), "External Apis");
        assert_eq!(
            super::split_pascal_case("FileLoggingFlags"),
            "File Logging Flags"
        );
    }

    // Tests all permutations of format_option

    #[test]
    fn test_format_option_with_short_and_long_and_values() {
        let arg = Arg::new("data_dir")
            .short('d')
            .long("data-dir")
            .value_name("DIR")
            .action(ArgAction::Set);

        assert_eq!(format_option(&arg), "`-d`, `--data-dir <DIR>`");
    }

    #[test]
    fn test_format_option_with_short_and_long_flag() {
        let arg = Arg::new("subscribe")
            .short('s')
            .long("subscribe")
            .action(ArgAction::SetTrue);

        assert_eq!(format_option(&arg), "`-s`, `--subscribe`");
    }

    #[test]
    fn test_format_option_with_long_only_and_values() {
        let arg = Arg::new("key_file")
            .long("key-file")
            .value_name("PATH")
            .action(ArgAction::Set);

        assert_eq!(format_option(&arg), "`--key-file <PATH>`");
    }

    #[test]
    fn test_format_option_with_long_only_flag() {
        let arg = Arg::new("http").long("http").action(ArgAction::SetTrue);

        assert_eq!(format_option(&arg), "`--http`");
    }

    #[test]
    fn test_format_option_with_short_only_and_values() {
        let arg = Arg::new("key")
            .short('k')
            .value_name("KEY")
            .action(ArgAction::Set);

        assert_eq!(format_option(&arg), "`-k <KEY>`");
    }

    #[test]
    fn test_format_option_with_short_only_flag() {
        let arg = Arg::new("verbose").short('v').action(ArgAction::SetTrue);

        assert_eq!(format_option(&arg), "`-v`");
    }

    #[test]
    fn test_format_option_positional_with_no_short_or_long() {
        let arg = Arg::new("input").value_name("VALUE").action(ArgAction::Set);

        assert_eq!(format_option(&arg), "`<VALUE>`");
    }

    #[test]
    fn test_format_option_value_name_default_uppercases_arg_value() {
        let arg = Arg::new("my_option")
            .long("my-option")
            .action(ArgAction::Set);

        assert_eq!(format_option(&arg), "`--my-option <MY_OPTION>`");
    }

    // Tests for character escaping requirements in format_description. Required to ensure
    // markdown tables render correctly.

    #[test]
    fn test_format_description_escapes_pipes_for_markdown_tables() {
        // Pipes must be escaped to `\|` so they don't break markdown table columns.
        let arg = Arg::new("choice")
            .long("choice")
            .action(ArgAction::Set)
            .help("Use A | B");

        let result = format_description(&arg).unwrap();
        assert_eq!(result, "Use A \\| B");
    }

    #[test]
    fn test_format_description_escapes_curly_braces() {
        // MDX format requires escaping curly braces.
        let arg = Arg::new("network")
            .long("network")
            .action(ArgAction::Set)
            .help("Defaults to {network}");

        let result = format_description(&arg).unwrap();
        assert_eq!(result, "Defaults to \\{network\\}");
    }

    #[test]
    fn test_format_description_collapses_newlines_double_spaces_to_single_spaces() {
        let arg = Arg::new("multi")
            .long("multi")
            .action(ArgAction::Set)
            .help("Line one\nLine  two");

        let result = format_description(&arg).unwrap();
        assert_eq!(result, "Line one Line two");
    }

    #[test]
    fn test_format_description_appends_possible_values_to_help_string() {
        #[derive(Clone, ValueEnum)]
        enum TestTopic {
            A,
            B,
        }
        let arg = Arg::new("format")
            .long("format")
            .action(ArgAction::Set)
            .value_parser(EnumValueParser::<TestTopic>::new()) // Equivalent to clap_derive for an enum type.
            .help("Output format");

        let result = format_description(&arg).unwrap();
        assert_eq!(result, "Output format (possible values: a, b)");
    }

    #[test]
    fn test_format_description_with_empty_help_returns_empty_string() {
        let arg = Arg::new("silent").long("silent");

        let result = format_description(&arg).unwrap();
        assert_eq!(result, "");
    }
}

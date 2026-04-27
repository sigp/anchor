//! Contains documentation generator help string formatting to produce markdown content for CLI
//! reference pages.
use std::fmt::Write;

use clap::{Arg, ArgAction};

use crate::errors::DocGenError;

/// A container for a group of CLI arguments that belong to a common semantic group.
type CliArgGrouping<'a> = Vec<&'a Arg>;

/// A container of CLI argument semantic groupings with optional group display names.
pub type GroupedCliArgs<'a> = Vec<(Option<String>, CliArgGrouping<'a>)>;

/// Helper function that converts a word to title-case format.
///
/// Outputs the string with the first litter capitalized.
pub(crate) fn to_title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Group arguments by the help heading registered for it.
///
/// Help headings are set on either the arg itself or on its parent CLI subcommand.
pub(crate) fn group_args_by_help_heading<'a>(args: &[&'a Arg]) -> GroupedCliArgs<'a> {
    let mut result: GroupedCliArgs<'a> = Vec::new();

    for arg in args {
        let heading = Some(
            arg.get_help_heading()
                .unwrap_or("Additional Options")
                .to_string(),
        );

        match result.iter_mut().find(|(name, _)| *name == heading) {
            Some(group) => group.1.push(arg),
            None => result.push((heading, vec![arg])),
        }
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
        return if arg.is_required_set() {
            "Required".to_string()
        } else {
            String::new()
        };
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

    use super::{format_description, format_option, group_args_by_help_heading};

    #[derive(Parser, Clone, Debug)]
    #[clap(next_help_heading = "Test Group 2")]
    pub struct TestFlag1 {
        #[clap(
            long,
            short = 't',
            value_name = "TEST",
            help = "Test flag 1.",
            help_heading = "Test Group 1",
            alias = "test1"
        )]
        pub test_flag_1: String,
        #[clap(
            long,
            short = 'u',
            value_name = "TEST_2",
            help = "Test flag 2.",
            alias = "test2"
        )]
        pub test_flag_2: String,
    }

    #[derive(Parser, Clone, Debug)]
    #[clap(name = "testcli", about = "Test cli interface.", next_line_help = true)]
    struct TestCli {
        #[clap(flatten)]
        pub test_flag_1: TestFlag1,
        #[clap(flatten)]
        pub test_flag_2: TestFlag1,
    }

    #[test]
    fn test_group_args_by_help_heading_outputs_correct_groupings() {
        let cmd = TestCli::command();
        let args: Vec<_> = cmd.get_arguments().collect();
        let result = group_args_by_help_heading(&args);

        // Check that we have both groups
        let group_names: Vec<(String, String)> = result
            .iter()
            .map(|(name, argument)| {
                (
                    name.as_ref().unwrap().to_string(),
                    argument[0].get_id().to_string(),
                )
            })
            .collect();
        assert_eq!(
            group_names[0],
            ("Test Group 1".to_string(), "test_flag_1".to_string()),
            "Group 1 and flag not found in result {:?}",
            group_names
        );
        assert_eq!(
            group_names[1],
            ("Test Group 2".to_string(), "test_flag_2".to_string()),
            "Group 2 and flag not found in result {:?}",
            group_names
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

    #[test]
    fn test_format_default_marks_required_without_default() {
        let arg = Arg::new("rpc")
            .long("rpc")
            .required(true)
            .action(ArgAction::Set);

        let result = super::format_default(&arg);
        assert_eq!(result, "Required");
    }

    #[test]
    fn test_to_title_case_capitalizes_first_letter() {
        assert_eq!(
            super::to_title_case("anchor"),
            "Anchor",
            "to_title_case must capitalize first letter"
        );
        assert_eq!(
            super::to_title_case("Anchor"),
            "Anchor",
            "to_title_case must leave already capitalized strings unchanged"
        );
    }
}

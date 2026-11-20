use std::any::TypeId;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Arg, ArgAction, Command};
use serde::Serialize;
use serde_json::{Map, Value, json};

const EXAMPLES_HEADER: &str = "Examples:";

pub fn example_help(examples: &[&str]) -> String {
    format!("{EXAMPLES_HEADER}\n  {}", examples.join("\n  "))
}

pub fn command_fields(command: &Command) -> Map<String, Value> {
    let (arguments, flags): (Vec<_>, Vec<_>) = command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set())
        .map(|arg| (arg.is_positional(), ArgumentInfo::from(arg)))
        .partition(|(positional, _)| *positional);
    let details = |args: Vec<_>| args.into_iter().map(|(_, info)| info).collect::<Vec<_>>();
    let children = command
        .get_subcommands()
        .filter(|child| !child.is_hide_set())
        .map(|child| Value::Object(command_fields(child)))
        .collect::<Vec<_>>();
    let summary = command
        .get_about()
        .map(ToString::to_string)
        .unwrap_or_default();
    let mut fields = Map::from_iter(
        [
            ("name", command.get_name().into()),
            ("summary", summary.into()),
            ("examples", json!(examples(command))),
            ("arguments", json!(details(arguments))),
            ("flags", json!(details(flags))),
            ("commands", children.into()),
        ]
        .map(|(key, value)| (key.into(), value)),
    );
    if command.get_name() == "config" {
        let variables: Vec<_> = crate::config::environment_variables().collect();
        fields.insert("environment".into(), json!(variables));
    }
    fields
}

pub fn examples(command: &Command) -> Vec<String> {
    let footer = command
        .get_after_long_help()
        .or_else(|| command.get_after_help());
    footer
        .map(ToString::to_string)
        .unwrap_or_default()
        .lines()
        .skip_while(|line| line.trim() != EXAMPLES_HEADER)
        .skip(1)
        .skip_while(|line| line.trim().is_empty())
        .take_while(|line| line.starts_with(char::is_whitespace) && !line.trim().is_empty())
        .map(|line| line.trim().to_owned())
        .collect()
}

pub fn argument_summary(arg: &Arg) -> Option<String> {
    arg.get_help()
        .or_else(|| arg.get_long_help())
        .map(ToString::to_string)
}

#[derive(Serialize)]
struct ArgumentInfo {
    name: String,
    long: Option<String>,
    short: Option<char>,
    summary: Option<String>,
    #[serde(rename = "type")]
    value_type: String,
    default: Value,
    env: Option<String>,
    choices: Vec<String>,
    global: bool,
    required: bool,
}

macro_rules! parser_is {
    ($parser:expr, $($kind:ty),+ $(,)?) => {
        $($parser == TypeId::of::<$kind>())||+
    };
}

impl From<&Arg> for ArgumentInfo {
    fn from(arg: &Arg) -> Self {
        let choices: Vec<_> = arg
            .get_possible_values()
            .iter()
            .filter(|choice| !choice.is_hide_set())
            .map(|choice| choice.get_name().to_owned())
            .collect();
        let parser = arg.get_value_parser().type_id();
        let value_type = match arg.get_action() {
            ArgAction::Count => "integer".into(),
            action if !action.takes_values() || parser == TypeId::of::<bool>() => "boolean".into(),
            _ if !choices.is_empty() => "enum".into(),
            _ if parser == TypeId::of::<Duration>() => "duration".into(),
            _ if parser == TypeId::of::<PathBuf>() => "path".into(),
            _ if parser_is!(parser, String, OsString) => "string".into(),
            _ if parser_is!(parser, f32, f64) => "number".into(),
            _ if parser_is!(
                parser, i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize
            ) =>
            {
                "integer".into()
            }
            _ => arg
                .get_value_names()
                .and_then(|names| names.first())
                .map_or_else(|| "string".into(), |name| name.to_ascii_lowercase()),
        };
        let default = match arg.get_action() {
            ArgAction::SetFalse => json!(true),
            ArgAction::Count => json!(0),
            action if !action.takes_values() => json!(false),
            _ => match arg.get_default_values() {
                [] => Value::Null,
                [value] => json!(value.to_string_lossy()),
                values => json!(
                    values
                        .iter()
                        .map(|value| value.to_string_lossy())
                        .collect::<Vec<_>>()
                ),
            },
        };
        Self {
            name: arg.get_id().to_string(),
            long: arg.get_long().map(str::to_owned),
            short: arg.get_short(),
            summary: argument_summary(arg),
            value_type,
            default,
            env: if arg.is_global_set() && arg.get_id() == "json" {
                Some(crate::flags::JSON_ENV.into())
            } else {
                arg.get_env()
                    .map(|name| name.to_string_lossy().into_owned())
            },
            choices,
            global: arg.is_global_set(),
            required: arg.is_required_set(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_footer_round_trips_into_help_metadata() {
        let invocations = ["ppn archive releases", "ppn archive releases --yes"];
        let footer = example_help(&invocations);
        assert_eq!(
            footer,
            "Examples:\n  ppn archive releases\n  ppn archive releases --yes"
        );
        let command = Command::new("archive").after_help(footer);
        assert_eq!(examples(&command), invocations);
    }

    #[test]
    fn reflects_positionals_and_skips_hidden_subcommands() {
        let mut command = Command::new("ppn")
            .disable_help_subcommand(true)
            .subcommand(Command::new("debug").hide(true))
            .subcommand(
                Command::new("archive")
                    .about("Archive a board.")
                    .arg(
                        Arg::new("board")
                            .required(true)
                            .long_help("Board to archive."),
                    )
                    .arg(
                        Arg::new("days")
                            .long("older-than")
                            .value_parser(clap::value_parser!(u32))
                            .default_value("30")
                            .env("PPN_ARCHIVE_AGE"),
                    )
                    .after_help(format!(
                        "{}\n\nNotes:\n  Unpin it first.",
                        example_help(&["ppn archive releases"])
                    )),
            );
        command.build();
        let tree = Value::Object(command_fields(&command));
        let commands = tree["commands"].as_array().unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0]["examples"], json!(["ppn archive releases"]));
        assert_eq!(commands[0]["arguments"][0]["name"], "board");
        assert_eq!(commands[0]["arguments"][0]["required"], true);
        assert_eq!(commands[0]["arguments"][0]["summary"], "Board to archive.");
        assert_eq!(commands[0]["flags"][0]["type"], "integer");
        assert_eq!(commands[0]["flags"][0]["default"], "30");
        assert_eq!(commands[0]["flags"][0]["env"], "PPN_ARCHIVE_AGE");
    }

    #[test]
    fn reflects_parser_types_without_relying_on_value_names() {
        let flags: [(clap::builder::ValueParser, &str); 5] = [
            (clap::value_parser!(i128).into(), "integer"),
            (clap::value_parser!(u16).into(), "integer"),
            (clap::value_parser!(f64).into(), "number"),
            (clap::value_parser!(PathBuf), "path"),
            (clap::value_parser!(OsString), "string"),
        ];
        for (parser, expected) in flags {
            let arg = Arg::new("threshold")
                .value_name("CUSTOM")
                .value_parser(parser);
            assert_eq!(ArgumentInfo::from(&arg).value_type, expected);
        }
    }
}

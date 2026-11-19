use std::env;
use std::ffi::OsString;
use std::time::Duration;

use clap::builder::{BoolishValueParser, TypedValueParser};
use clap::{Args, ValueEnum};

use crate::terminal::Color;

const JSON_ENV: &str = "PPN_JSON";

#[derive(Args)]
pub struct Globals {
    /// Render output as JSON.
    #[arg(long, global = true, env = JSON_ENV, value_parser = BoolishValueParser::new())]
    pub json: bool,

    /// Confirm without prompting.
    #[arg(long, global = true)]
    pub yes: bool,

    /// Limit the time spent waiting, for example 30s or 2m.
    #[arg(long, global = true, value_name = "DURATION", value_parser = humantime::parse_duration)]
    pub timeout: Option<Duration>,

    /// Control color on a terminal. Honors NO_COLOR.
    #[arg(long, global = true, default_value = "auto", value_enum)]
    pub color: Color,
}

impl Globals {
    pub fn json_on_early_exit(args: &[OsString], command: &clap::Command) -> bool {
        options(args).iter().any(|arg| arg == "--json")
            || env::var_os(JSON_ENV).is_some_and(|value| {
                BoolishValueParser::new()
                    .parse_ref(command, None, &value)
                    .is_ok_and(|enabled| enabled)
            })
    }
}

pub fn color_on_early_exit(args: &[OsString]) -> Color {
    let args = options(args);
    args.iter()
        .enumerate()
        .filter_map(|(index, arg)| match arg.to_str()? {
            "--color" => args.get(index + 1)?.to_str(),
            arg => arg.strip_prefix("--color="),
        })
        .filter_map(|value| Color::from_str(value, false).ok())
        .next_back()
        .unwrap_or_default()
}

fn options(args: &[OsString]) -> &[OsString] {
    let tail = args.get(1..).unwrap_or_default();
    let end = tail
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(tail.len());
    &tail[..end]
}

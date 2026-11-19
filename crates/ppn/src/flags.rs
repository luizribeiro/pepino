use std::env;
use std::ffi::OsString;
use std::time::Duration;

use clap::{Args, ValueEnum};

use crate::error::{CliError, ErrorKind};
use crate::terminal::Color;

const JSON_ENV: &str = "PPN_JSON";

#[derive(Args)]
pub struct Globals {
    /// Render output as JSON.
    #[arg(long, global = true, env = JSON_ENV, value_parser = parse_json)]
    pub json: bool,

    /// Confirm without prompting.
    #[arg(long, global = true)]
    pub yes: bool,

    /// Limit the time spent waiting, for example 5 seconds, 500ms, or 2m.
    #[arg(long, global = true, value_name = "DURATION", value_parser = parse_timeout)]
    pub timeout: Option<Duration>,

    /// Control color on a terminal. Honors NO_COLOR.
    #[arg(long, global = true, default_value = "auto", value_enum)]
    pub color: Color,
}

fn parse_timeout(value: &str) -> Result<Duration, humantime::DurationError> {
    let value = value.trim();
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return humantime::parse_duration(&format!("{value}s"));
    }
    humantime::parse_duration(value)
}

fn parse_json(value: &str) -> Result<bool, &'static str> {
    match value {
        "" | "0" | "false" => Ok(false),
        "1" | "true" => Ok(true),
        _ => Err("expected 0, false, 1, or true"),
    }
}

pub fn json_from_env() -> Result<bool, CliError> {
    let value = env::var_os(JSON_ENV).unwrap_or_default();
    value
        .to_str()
        .ok_or("expected a UTF-8 value")
        .and_then(parse_json)
        .map_err(|error| CliError::new(ErrorKind::Usage, format!("Invalid {JSON_ENV}: {error}")))
}

pub fn json_on_command_line(args: &[OsString]) -> bool {
    options(args).iter().any(|arg| arg == "--json")
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

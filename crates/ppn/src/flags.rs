use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, ValueEnum};

use crate::config::{Env, LogLevel, env_value};
use crate::error::{CliError, ErrorKind};
use crate::terminal::Color;

pub(crate) const JSON_ENV: &str = "PPN_JSON";

#[derive(Args)]
pub struct Globals {
    /// Render output as JSON. PPN_JSON also enables this.
    #[arg(long, global = true)]
    pub json: bool,

    /// Confirm without prompting.
    #[arg(long, global = true)]
    pub yes: bool,

    /// Limit the time spent waiting, for example 5 seconds, 500ms, or 2m.
    #[arg(long, global = true, value_name = "DURATION", value_parser = parse_timeout)]
    pub timeout: Option<Duration>,

    /// Set the daemon log level.
    #[arg(long, global = true, value_enum)]
    pub log_level: Option<LogLevel>,

    /// Use this file in place of the user and system files. PPN_CONFIG also selects a file.
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Control color. Auto honors NO_COLOR and the terminal.
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
        "0" | "false" => Ok(false),
        "1" | "true" => Ok(true),
        _ => Err("expected 0, false, 1, or true"),
    }
}

pub fn json_from_env(env: &Env) -> Result<bool, CliError> {
    env_value(env, JSON_ENV)
        .map_err(|error| error.to_string())
        .and_then(|value| value.map_or(Ok(false), parse_json).map_err(str::to_owned))
        .map_err(|error| CliError::new(ErrorKind::Usage, format!("Invalid {JSON_ENV}: {error}")))
}

pub fn json_on_command_line(args: &[OsString]) -> bool {
    options(args).iter().any(|arg| arg == "--json")
}

impl Globals {
    pub fn on_early_exit(args: &[OsString], json: bool) -> Self {
        Self {
            json,
            config: None,
            log_level: None,
            yes: options(args).iter().any(|arg| arg == "--yes"),
            color: option_values(args, "--color")
                .filter_map(|value| Color::from_str(value, false).ok())
                .next_back()
                .unwrap_or_default(),
            timeout: option_values(args, "--timeout")
                .filter_map(|value| parse_timeout(value).ok())
                .next_back(),
        }
    }
}

fn option_values<'a>(
    args: &'a [OsString],
    option: &'a str,
) -> impl DoubleEndedIterator<Item = &'a str> {
    let args = options(args);
    args.iter()
        .enumerate()
        .filter_map(move |(index, arg)| match arg.to_str()? {
            arg if arg == option => args.get(index + 1)?.to_str(),
            arg => arg.strip_prefix(option)?.strip_prefix('='),
        })
}

fn options(args: &[OsString]) -> &[OsString] {
    let tail = args.get(1..).unwrap_or_default();
    let end = tail
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(tail.len());
    &tail[..end]
}

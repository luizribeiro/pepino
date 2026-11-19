use std::env;
use std::ffi::OsString;
use std::time::Duration;

use clap::Args;
use clap::builder::{BoolishValueParser, TypedValueParser};

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
    pub fn json_on_parse_error(args: &[OsString], command: &clap::Command) -> bool {
        args.iter()
            .skip(1)
            .take_while(|arg| *arg != "--")
            .any(|arg| arg == "--json")
            || env::var_os(JSON_ENV).is_some_and(|value| {
                BoolishValueParser::new()
                    .parse_ref(command, None, &value)
                    .is_ok_and(|enabled| enabled)
            })
    }
}

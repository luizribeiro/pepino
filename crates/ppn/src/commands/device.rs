use std::path::Path;

use clap::Subcommand;
use pepino_app::{App, OpenOptions};
use ppn::{
    device_code::{Error as CodeError, Kind},
    error::{CliError, ErrorKind},
    help_tree::example_help,
    output::{CommandResult, Render},
    terminal::{Context, Terminal},
};
use serde_json::{Map, Value, json};

#[derive(clap::Args)]
#[command(
    about = "Link your devices using a manual request and grant.",
    long_about = "Link your devices using a manual request and grant. Device removal and revocation, one-code linking, merging existing identities, and board relay management are outside this version.",
    after_help = example_help(&["ppn device request --name ferrum"])
)]
pub struct Args {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Prepare an uninitialised host and print its reusable request code.
    #[command(after_help = example_help(&["ppn device request --name ferrum", "ppn device request --json"]))]
    Request {
        /// Name for the requesting device. Defaults to the hostname.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
    },
}

pub fn run(
    context: &Context<'_>,
    args: Args,
    _selection: Option<&Path>,
    _allow_start: bool,
) -> Result<CommandResult, CliError> {
    match args.action {
        Action::Request { name } => {
            let (directory, options) = local_options(context, name)?;
            let request = runtime()?.block_on(App::request_device(directory, options))?;
            let code = Kind::Request.encode(&request).map_err(invalid_code)?;
            Ok(CommandResult::document(RequestCode(code)))
        }
    }
}

fn local_options<'a>(
    context: &'a Context<'_>,
    name: Option<String>,
) -> Result<(&'a Path, OpenOptions), CliError> {
    let config = &context.config()?.config;
    let mut options = OpenOptions {
        device_name: name,
        credentials_file: Some(config.credentials_file.value.as_path().to_owned()),
        ..Default::default()
    };
    if let Some(timeout) = context.timeout {
        options.lock_timeout = timeout;
    }
    Ok((config.data_dir.value.as_path(), options))
}

fn runtime() -> Result<tokio::runtime::Runtime, CliError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| {
            CliError::new(
                ErrorKind::Failure,
                "Could not start the device command runtime.",
            )
        })
}

fn invalid_code(error: CodeError) -> CliError {
    CliError::new(ErrorKind::InvalidInput, error.to_string())
}

struct RequestCode(String);

impl Render for RequestCode {
    fn human(&self, _: &Terminal) -> String {
        self.0.clone()
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("device_request")),
            ("request".into(), json!(self.0)),
        ])
    }
}

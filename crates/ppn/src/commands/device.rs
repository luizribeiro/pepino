mod add;
mod code_input;
mod join;
mod ls;

use std::path::Path;

use clap::Subcommand;
use pepino_app::{App, OpenOptions};
use pepino_core::DeviceId;
use ppn::{
    device_code::{Error as CodeError, Kind},
    error::{CliError, ErrorKind},
    help_tree::example_help,
    output::{CommandResult, Render},
    render::single_line,
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
    /// List this identity's devices and their membership status.
    #[command(after_help = example_help(&["ppn device ls", "ppn device ls --json"]))]
    Ls,

    /// Install a grant and wait for board membership and the first sync.
    #[command(after_help = example_help(&["ppn device join - --timeout 45s < grant.txt", "ppn device join --from-file grant.txt"]))]
    Join {
        #[command(flatten)]
        input: code_input::CodeInput,
    },

    /// Grant a requested device manager access to your boards and print a secret code.
    #[command(after_help = example_help(&["ppn device add - --yes < request.txt", "ppn device add --from-file request.txt --yes"]))]
    Add {
        #[command(flatten)]
        input: code_input::CodeInput,
        /// Override the requesting device's name.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
    },

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
    selection: Option<&Path>,
    allow_start: bool,
) -> Result<CommandResult, CliError> {
    match args.action {
        Action::Ls => ls::run(context),
        Action::Join { input } => join::run(context, &input.read(context)?, selection),
        Action::Add { input, name } => {
            add::run(context, &input.read(context)?, name, selection, allow_start)
        }
        Action::Request { name } => {
            let (directory, options) = local_options(context, name)?;
            let request = runtime()?.block_on(App::request_device(directory, options))?;
            let code = Kind::Request.encode(&request).map_err(invalid_code)?;
            let fingerprint = request.fingerprint();
            let note = format!(
                "Device {}, fingerprint {fingerprint}. This request code is secret.",
                single_line(&request.name),
            );
            Ok(CommandResult::document(RequestCode {
                code,
                fingerprint,
                id: request.id,
            })
            .with_human_note(Some(note)))
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

struct RequestCode {
    code: String,
    fingerprint: String,
    id: DeviceId,
}

impl Render for RequestCode {
    fn human(&self, _: &Terminal) -> String {
        self.code.clone()
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("device_request")),
            ("request".into(), json!(self.code)),
            ("fingerprint".into(), json!(self.fingerprint)),
            ("device_id".into(), json!(self.id)),
            ("secret".into(), json!(true)),
        ])
    }
}

fn unexpected_reply() -> CliError {
    CliError::new(
        ErrorKind::DaemonUnreachable,
        "The daemon returned an unexpected control response.",
    )
}

mod add;
mod code_input;
mod join;
mod ls;
mod rename;
mod revoke;

use std::path::Path;

use clap::Subcommand;
use pepino_app::{App, OpenOptions};
use pepino_core::DeviceId;
use ppn::{
    device_code::{Error as CodeError, Kind, encode_request},
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
    long_about = "Link your devices using a manual request and grant. One-code linking, merging existing identities, and board relay management are outside this version.",
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

    /// Revoke another device from this identity and remove its board access.
    #[command(after_help = example_help(&["ppn device revoke 'Garden tablet'", "ppn device revoke 97ad97 --yes --json"]))]
    Revoke {
        /// Device name, full ID, or unambiguous ID prefix.
        #[arg(value_name = "DEVICE")]
        device: String,
    },

    /// Rename this device or another device in the identity.
    #[command(override_usage = "ppn device rename [OPTIONS] [DEVICE] <NAME>", after_help = example_help(&["ppn device rename Titanium", "ppn device rename Ferrum 'Garden tablet'"]))]
    Rename {
        /// New name, or a device name or ID when followed by a new name.
        device_or_name: String,
        /// New name when targeting a specific device.
        name: Option<String>,
    },

    /// Install a grant and wait for board membership and the first sync.
    #[command(after_help = example_help(&["ppn device join - --timeout 45s < grant.txt", "ppn device join --from-file grant.txt"]))]
    Join {
        #[command(flatten)]
        input: code_input::CodeInput,
    },

    /// Grant a requested device manager access to your boards and print a secret code.
    #[command(after_help = example_help(&["ppn device add - --fingerprint 9XF5-HK7M-QS6V-RY24 < request.txt", "ppn device add --from-file request.txt --yes"]))]
    Add {
        #[command(flatten)]
        input: code_input::CodeInput,
        /// Confirm the fingerprint shown on the requesting device.
        #[arg(long, value_name = "FP")]
        fingerprint: Option<String>,
        /// Override the requesting device's name.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
    },

    /// Prepare an uninitialised host and print its reusable request code.
    #[command(after_help = example_help(&["ppn device request --name ferrum", "ppn device request --cancel", "ppn device request --json"]))]
    Request {
        /// Remove the pending request and its node state so this host can initialise.
        #[arg(long, conflicts_with = "name")]
        cancel: bool,
        /// Name for the requesting device. Defaults to the hostname.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
    },
}

impl Args {
    pub(crate) fn requires_identity(&self) -> bool {
        matches!(
            self.action,
            Action::Ls | Action::Add { .. } | Action::Rename { .. } | Action::Revoke { .. }
        )
    }
}

pub fn run(
    context: &Context<'_>,
    args: Args,
    selection: Option<&Path>,
    allow_start: bool,
) -> Result<CommandResult, CliError> {
    match args.action {
        Action::Ls => ls::run(context),
        Action::Revoke { device } => revoke::run(context, &device),
        Action::Rename {
            device_or_name,
            name,
        } => rename::run(context, device_or_name, name),
        Action::Join { input } => join::run(context, &input.read(context)?, selection),
        Action::Add {
            input,
            name,
            fingerprint,
        } => add::run(
            context,
            &input.read(context)?,
            name,
            fingerprint.as_deref(),
            selection,
            allow_start,
        ),
        Action::Request { name, cancel } => {
            let (directory, options) = local_options(context, name)?;
            let runtime = runtime()?;
            if cancel {
                let removed = runtime.block_on(App::cancel_device_request(directory, options))?;
                return Ok(CommandResult::document(CancelRequest(removed)));
            }
            let request = runtime.block_on(App::request_device(directory, options))?;
            let code = encode_request(&request)?;
            let fingerprint = request.fingerprint();
            let expires_at = i64::try_from(request.expires_at()?)
                .ok()
                .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
                .ok_or_else(|| {
                    CliError::new(ErrorKind::InvalidInput, "Invalid device request expiry.")
                })?;
            let settings = context.render_settings()?;
            let expiry = settings.dates().timestamp(expires_at, &settings.terminal);
            let note = format!(
                "Device {}, fingerprint {fingerprint}. This request code is secret. Expires {expiry}.",
                single_line(&request.name),
            );
            Ok(CommandResult::document(RequestCode {
                code,
                fingerprint,
                id: request.id,
                expires_at,
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
    expires_at: chrono::DateTime<chrono::Utc>,
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
            ("expires_at".into(), json!(self.expires_at)),
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

struct CancelRequest(bool);

impl Render for CancelRequest {
    fn human(&self, _: &Terminal) -> String {
        if self.0 {
            "Device request cancelled."
        } else {
            "No device request was pending."
        }
        .into()
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("device_request_cancel")),
            ("cancelled".into(), json!(self.0)),
        ])
    }
}

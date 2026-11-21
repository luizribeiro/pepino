use std::{path::Path, time::Duration};

use ppn::{
    daemon::{
        address::socket_path,
        client::Client,
        protocol::{Operation, Outcome},
    },
    device_code::Kind,
    error::{CliError, ErrorKind},
    output::CommandResult,
    render::single_line,
    terminal::Context,
};

use crate::commands::{
    daemon::{ensure_running, unreachable},
    device::{RequestCode, invalid_code},
};

pub(super) fn run(
    context: &Context<'_>,
    name: Option<String>,
    config_path: Option<&Path>,
    allow_start: bool,
) -> Result<CommandResult, CliError> {
    let config = &context.config()?.config;
    pepino_app::LocalState::load(config.data_dir.value.as_path())?.check_device_write()?;
    let client = running(context, config_path, allow_start)?;
    let outcome = client
        .call_with(Operation::BoardRequest { name }, Duration::from_secs(20))
        .map_err(unreachable)?;
    let request = match outcome {
        Outcome::BoardRequest { request } => request,
        Outcome::Error { code, message } => return Err(control_error(&code, message)),
        _ => return Err(crate::commands::device::unexpected_reply()),
    };
    let code = Kind::BoardRequest.encode(&request).map_err(invalid_code)?;
    let fingerprint = request.fingerprint()?;
    let expires_at = chrono::DateTime::from_timestamp(request.expires_at()? as i64, 0)
        .ok_or_else(|| CliError::new(ErrorKind::InvalidInput, "Invalid board request expiry."))?;
    let settings = context.render_settings()?;
    let expiry = settings.dates().timestamp(expires_at, &settings.terminal);
    let note = format!(
        "Device {}, fingerprint {fingerprint}. This request code is secret. Expires {expiry}.",
        single_line(&request.device.name)
    );
    Ok(CommandResult::document(RequestCode {
        kind: "board_request",
        code,
        fingerprint,
        id: request.device.id,
        expires_at,
    })
    .with_human_note(Some(note)))
}

pub(super) fn control_error(code: &str, message: String) -> CliError {
    let kind = ErrorKind::all()
        .iter()
        .copied()
        .find(|kind| kind.code() == code)
        .unwrap_or(ErrorKind::Failure);
    CliError::new(kind, message)
}

pub(super) fn running(
    context: &Context<'_>,
    config_path: Option<&Path>,
    allow_start: bool,
) -> Result<Client, CliError> {
    let config = &context.config()?.config;
    let client = Client {
        path: socket_path(config),
    };
    ensure_running(
        &client,
        context,
        config_path,
        allow_start && config.daemon.autostart.value,
    )?;
    Ok(client)
}

use std::{path::Path, time::Duration};

use pepino_app::{BoardRequest, BoardRole};
use pepino_node::DeviceCredentials;
use ppn::{
    app::AppClient,
    daemon::protocol::{Operation, Outcome},
    device_code::Kind,
    error::{CliError, ErrorKind},
    output::CommandResult,
    render::{short_id, single_line},
    terminal::Context,
};

use crate::commands::{
    daemon::unreachable,
    device::{
        add::{GrantCode, confirm},
        invalid_code, unexpected_reply,
    },
};

pub(super) fn run(
    context: &Context<'_>,
    code: &str,
    role: BoardRole,
    expected: Option<&str>,
    config_path: Option<&Path>,
    allow_start: bool,
) -> Result<CommandResult, CliError> {
    let mut request: BoardRequest = Kind::BoardRequest.decode(code).map_err(invalid_code)?;
    let name = request.name()?;
    let secret = request.secret()?;
    let app = AppClient::open(context)?;
    let board = app.selected_board()?.0;
    let local = app.local_state();
    local.check_invitable(board.id)?;
    local.check_person_id(request.identity.key())?;
    let fingerprint = request.fingerprint()?;
    let binding = request.binding()?;
    let notice = format!(
        "Inviting {} ({}), device {}, fingerprint {fingerprint}, to board {} as {role}",
        single_line(&name),
        short_id(&request.identity.id.to_string()),
        single_line(&request.device.name),
        single_line(&board.name)
    );
    confirm(context, expected, &fingerprint, &notice)?;
    let config = &context.config()?.config;
    let keys = DeviceCredentials::load(config.credentials_file.value.as_path())
        .ok()
        .filter(|keys| keys.device_id() == local.device_id)
        .ok_or_else(|| {
            CliError::new(
                ErrorKind::Failure,
                "Could not load the granting device credentials.",
            )
        })?;
    let daemon = super::request::running(context, config_path, allow_start)?;
    request.device.secret = None;
    let outcome = daemon
        .call_with(
            Operation::BoardInvite {
                board: board.id,
                request: Box::new(request.clone()),
                role,
                relay_url: config
                    .relay_urls
                    .value
                    .first()
                    .map(|relay| relay.as_url().to_string()),
            },
            Duration::from_secs(20),
        )
        .map_err(unreachable)?;
    let mut grant = match outcome {
        Outcome::BoardInvited { grant } => grant,
        Outcome::MemberAccessDenied { board } => {
            return Err(CliError::new(
                ErrorKind::PermissionDenied,
                format!(
                    "Inviting someone requires the manage role on board {:?}.",
                    single_line(&board)
                ),
            ));
        }
        Outcome::Error { code, message } => {
            return Err(super::request::control_error(&code, message));
        }
        _ => return Err(unexpected_reply()),
    };
    request.device.secret = Some(secret);
    // The request's secret contributes to the binding, which has a fixed encoded size.
    grant.request = binding;
    let signed = grant.issue(&request, &keys)?;
    let code = Kind::BoardGrant.encode(&signed).map_err(invalid_code)?;
    Ok(CommandResult::document(GrantCode {
        kind: "board_grant",
        code,
        fingerprint,
        id: grant.recipient,
    })
    .with_human_note(Some(format!(
        "{notice}\nThis grant code is secret. It contains the board ID."
    ))))
}

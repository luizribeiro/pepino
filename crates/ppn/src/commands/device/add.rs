use std::{path::Path, time::Duration};

use pepino_app::{DeviceGrant, DeviceRequest, PeerHint, SignedGrant};
use pepino_node::DeviceCredentials;
use ppn::{
    app::AppClient,
    daemon::{
        address::socket_path,
        client::Client,
        protocol::{Operation, Outcome},
    },
};

use super::*;
use crate::commands::daemon::{ensure_running, unreachable};

pub(super) fn run(
    context: &Context<'_>,
    code: &str,
    name: Option<String>,
    expected_fingerprint: Option<&str>,
    selection: Option<&Path>,
    allow_start: bool,
) -> Result<CommandResult, CliError> {
    let device: DeviceRequest = Kind::Request.decode(code).map_err(invalid_code)?;
    device.member_info()?;
    if device.secret.is_none() {
        return Err(CliError::new(
            ErrorKind::InvalidInput,
            "The request is missing its secret.",
        ));
    }
    if name.as_deref().is_some_and(|name| name.trim().is_empty()) {
        return Err(CliError::new(
            ErrorKind::InvalidInput,
            "The device name must contain a non-whitespace character.",
        ));
    }
    let fingerprint = device.fingerprint();
    let notice = format!(
        "Adding {}, fingerprint {fingerprint}",
        single_line(&device.name)
    );
    confirm(context, expected_fingerprint, &fingerprint, &notice)?;
    let config = &context.config()?.config;
    let app = AppClient::open(context)?;
    if app.local_state().device_id == device.id {
        return Err(CliError::new(
            ErrorKind::InvalidInput,
            "The request must come from another device.",
        ));
    }
    let keys = DeviceCredentials::load(config.credentials_file.value.as_path())
        .ok()
        .filter(|keys| keys.device_id() == app.local_state().device_id)
        .ok_or_else(|| {
            CliError::new(
                ErrorKind::Failure,
                "Could not load the granting device credentials.",
            )
        })?;
    let client = Client {
        path: socket_path(config),
    };
    ensure_running(
        &client,
        context,
        selection,
        allow_start && config.daemon.autostart.value,
    )?;
    let request = device.binding();
    let recipient = device.id;
    let outcome = client
        .call_with(
            Operation::AddMember {
                device: device.clone(),
                name,
            },
            Duration::from_secs(20),
        )
        .map_err(unreachable)?;
    if !matches!(outcome, Outcome::MemberAdded) {
        return Err(unexpected_reply());
    }

    let updated = AppClient::open(context)?;
    let local = updated.local_state();
    let grant = DeviceGrant {
        recipient,
        request,
        identity: local.identity_id,
        display_name: local.display_name.clone(),
        boards: local.boards().to_vec(),
        devices: local.devices(),
        peer: PeerHint {
            id: local.device_id,
            relay_url: config
                .relay_urls
                .value
                .first()
                .map(|relay| relay.as_url().to_string()),
        },
    };
    let signed = SignedGrant::issue(&grant, &device, &keys)?;
    let code = Kind::Grant.encode(&signed).map_err(invalid_code)?;
    let note = format!("{notice}\nThis grant code is secret. It contains your board IDs.");
    Ok(CommandResult::document(GrantCode {
        code,
        fingerprint,
        id: recipient,
    })
    .with_human_note(Some(note)))
}

fn confirm(
    context: &Context<'_>,
    expected: Option<&str>,
    actual: &str,
    notice: &str,
) -> Result<(), CliError> {
    if let Some(expected) = expected {
        if expected.eq_ignore_ascii_case(actual) {
            return Ok(());
        }
        return Err(CliError::new(
            ErrorKind::InvalidInput,
            format!("{notice}. The supplied fingerprint does not match."),
        ));
    }
    if !context.yes && !context.terminal.stdin_is_tty {
        return Err(CliError::new(ErrorKind::ConfirmationRequired, notice)
            .with_hint("Compare the fingerprint on the requesting device, then pass --fingerprint <FP> or --yes."));
    }
    if !context.confirm(notice)? {
        return Err(CliError::new(
            ErrorKind::ConfirmationRequired,
            "Device addition cancelled.",
        ));
    }
    Ok(())
}

struct GrantCode {
    code: String,
    fingerprint: String,
    id: DeviceId,
}

impl Render for GrantCode {
    fn human(&self, _: &Terminal) -> String {
        self.code.clone()
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("device_grant")),
            ("grant".into(), json!({"code": self.code, "secret": true})),
            ("fingerprint".into(), json!(self.fingerprint)),
            ("device_id".into(), json!(self.id)),
        ])
    }
}

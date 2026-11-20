use std::{path::Path, time::Duration};

use pepino_app::{DeviceGrant, DeviceRequest, PeerHint};
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
    selection: Option<&Path>,
    allow_start: bool,
) -> Result<CommandResult, CliError> {
    let device: DeviceRequest = Kind::Request.decode(code).map_err(invalid_code)?;
    device.member_info()?;
    if name.as_deref().is_some_and(|name| name.trim().is_empty()) {
        return Err(CliError::new(
            ErrorKind::InvalidInput,
            "The device name must contain a non-whitespace character.",
        ));
    }
    let config = &context.config()?.config;
    let app = AppClient::open(context)?;
    if app.local_state().device_id == device.id {
        return Err(CliError::new(
            ErrorKind::InvalidInput,
            "The request must come from another device.",
        ));
    }
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
            Operation::AddMember { device, name },
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
    let code = Kind::Grant.encode(&grant).map_err(invalid_code)?;
    let note = "This grant code is secret. It contains your board IDs.";
    Ok(CommandResult::document(GrantCode(code)).with_human_note(Some(note.into())))
}

struct GrantCode(String);

impl Render for GrantCode {
    fn human(&self, _: &Terminal) -> String {
        self.0.clone()
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("device_grant")),
            ("grant".into(), json!({"code": self.0, "secret": true})),
        ])
    }
}

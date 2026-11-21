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
    let device = ppn::device_code::decode_request(code)?;
    device.member_info()?;
    pepino_app::normalize_name(name.as_deref().unwrap_or(&device.name))?;
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
                device: DeviceRequest {
                    secret: None,
                    ..device.clone()
                },
                name,
            },
            Duration::from_secs(20),
        )
        .map_err(unreachable)?;
    if let Outcome::MemberAddIncomplete {
        enrolled,
        failed,
        causes,
    } = &outcome
    {
        let rows = |names: &[String]| {
            names
                .iter()
                .map(|name| single_line(name))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut message = format!(
            "Could not enroll the device on {}. Enrollment succeeded on {}.",
            rows(failed),
            if enrolled.is_empty() {
                "no boards".into()
            } else {
                rows(enrolled)
            }
        );
        for (board, cause) in failed.iter().zip(causes) {
            message.push_str(&format!(" {}: {}", single_line(board), single_line(cause)));
        }
        return Err(CliError::new(ErrorKind::Failure, message)
            .with_detail("enrolled_boards", json!(enrolled))
            .with_detail("failed_boards", json!(failed))
            .with_detail("causes", json!(causes)));
    }
    if let Outcome::MemberAccessDenied { board } = &outcome {
        return Err(CliError::new(
            ErrorKind::PermissionDenied,
            format!(
                "This device needs manage access to board {:?} before adding a device.",
                single_line(board)
            ),
        ));
    }
    if let Outcome::Error { message, .. } = outcome {
        return Err(CliError::new(ErrorKind::Failure, message));
    }
    if !matches!(outcome, Outcome::MemberAdded) {
        return Err(unexpected_reply());
    }

    let updated = AppClient::open(context)?;
    let local = updated.local_state();
    let grant = DeviceGrant {
        recipient,
        request,
        identity: local.identity_id,
        display_name: local.display_name().to_owned(),
        boards: local.identity_owned_boards().cloned().collect(),
        devices: local
            .devices()
            .into_iter()
            .filter(|(id, _)| *id == local.device_id || *id == recipient)
            .collect(),
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
        kind: "device_grant",
        code,
        fingerprint,
        id: recipient,
    })
    .with_human_note(Some(note)))
}

pub(crate) fn confirm(
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
            "Invitation cancelled.",
        ));
    }
    Ok(())
}

pub(crate) struct GrantCode {
    pub(crate) kind: &'static str,
    pub(crate) code: String,
    pub(crate) fingerprint: String,
    pub(crate) id: DeviceId,
}

impl Render for GrantCode {
    fn human(&self, _: &Terminal) -> String {
        self.code.clone()
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!(self.kind)),
            ("grant".into(), json!({"code": self.code, "secret": true})),
            ("fingerprint".into(), json!(self.fingerprint)),
            ("device_id".into(), json!(self.id)),
        ])
    }
}

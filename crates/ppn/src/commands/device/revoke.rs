use std::time::Duration;

use chrono::Utc;
use pepino_sync::{BoardRemoval, Removal};
use ppn::{
    app::AppClient,
    daemon::{
        address::socket_path,
        client::Client,
        protocol::{Operation, Outcome, wait_millis},
    },
    render::short_id,
};

use super::*;

const CONSEQUENCES: &str = "Once removed, the device can no longer read anything new. It keeps everything it already had. If offline, it may never receive notice and will show stale data. Adding it again gives it the full history, including the time it was removed. Concurrent revocations between managers can leave a board without a manager.";

const CONTEST_NOTE: &str = "If the revoked device contests, this device is revoked too.";
const ROOT_WARNING: &str =
    "The first device wins a contested revocation. If it is stolen, create a new identity.";

pub(super) fn run(context: &Context<'_>, reference: &str) -> Result<CommandResult, CliError> {
    let client = AppClient::open(context)?;
    let local = client.local_state();
    local.check_device_write()?;
    let target = local.revocation_target(reference)?;
    let name = single_line(&target.name);
    let id = target.id.to_string();
    let fingerprint = blake3::derive_key("Pepino device key fingerprint v1", target.id.as_bytes());
    let fingerprint = hex::encode(&fingerprint[..8]);
    let boards = local
        .boards()
        .iter()
        .map(|board| single_line(&board.name))
        .collect::<Vec<_>>();
    let root = local.roster_root();
    let notices: Vec<_> = [
        (root.is_some_and(|root| root != local.device_id)).then_some(CONTEST_NOTE),
        (root == Some(target.id)).then_some(ROOT_WARNING),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut prompt = format!(
        "Revoke {name} ({})? Key fingerprint {fingerprint}.\nBoards affected: {}.\n{CONSEQUENCES}",
        short_id(&id),
        boards.join(", "),
    );
    for notice in &notices {
        prompt.push('\n');
        if *notice == ROOT_WARNING {
            prompt.push_str("Warning: ");
        }
        prompt.push_str(notice);
    }
    let mut result = Report {
        device: target.id,
        name,
        confirmed: false,
        boards: Vec::new(),
        notices,
    };
    if !context.confirm(&prompt)? {
        return Ok(CommandResult::document(result));
    }
    let wait = context.timeout.unwrap_or(Duration::from_secs(8));
    let timeout_ms = wait_millis(wait)?;
    let (directory, options) = local_options(context, None)?;
    let local = runtime()?.block_on(async {
        let app = App::open_with_options(directory, context.time.now_ms, options).await?;
        app.record_device_revocation(target.id, Utc::now()).await
    })?;
    let daemon = Client {
        path: socket_path(&context.config()?.config),
    };
    let outcome = daemon.call_with(
        Operation::DeviceRemoval {
            device: target.id,
            timeout_ms,
        },
        wait + Duration::from_secs(1),
    );
    result.confirmed = true;
    result.boards = match outcome {
        Ok(Outcome::DeviceRemoval {
            boards: Some(boards),
        }) => boards,
        _ => local
            .boards()
            .iter()
            .filter(|board| {
                local.shared_access(board.id).is_none()
                    || local
                        .membership_devices(board.id, local.own_person_key())
                        .contains(&target.id)
            })
            .cloned()
            .map(|board| BoardRemoval {
                result: if local.waiting_for_access(board.id) {
                    Removal::WaitingForAccess
                } else {
                    Removal::Unavailable
                },
                board,
            })
            .collect(),
    };
    let mut output = CommandResult::document(result);
    if context.yes {
        output = output
            .with_human_note(
                root.is_some_and(|root| root != local.device_id)
                    .then(|| CONTEST_NOTE.to_owned()),
            )
            .with_warnings((root == Some(target.id)).then(|| ROOT_WARNING.to_owned()));
    }
    Ok(output)
}

struct Report {
    device: DeviceId,
    name: String,
    confirmed: bool,
    boards: Vec<BoardRemoval>,
    notices: Vec<&'static str>,
}

impl Report {
    fn complete(&self) -> bool {
        self.confirmed
            && self
                .boards
                .iter()
                .all(|board| board.result == Removal::Removed)
    }
}

impl Render for Report {
    fn human(&self, _: &Terminal) -> String {
        let mut text = if self.complete() {
            format!(
                "Revoked {} ({}).",
                self.name,
                short_id(&self.device.to_string())
            )
        } else if self.confirmed {
            format!(
                "Revocation recorded for {} ({}). Transport removal incomplete.",
                self.name,
                short_id(&self.device.to_string())
            )
        } else {
            return "Revocation cancelled.".into();
        };
        for outcome in &self.boards {
            let status = match outcome.result {
                Removal::Removed => "removed",
                Removal::ManagerRequired => "pending, this device needs Manage",
                Removal::LastManager => {
                    "pending, grant Manage to another device on this board first"
                }
                Removal::WaitingForAccess => {
                    "pending, this device is waiting for access. A board manager can remove it"
                }
                Removal::Unavailable => "pending, daemon or board unavailable",
            };
            text.push_str(&format!(
                "\n  {} · {status}",
                single_line(&outcome.board.name)
            ));
        }
        if self
            .boards
            .iter()
            .any(|board| board.result != Removal::Removed)
        {
            text.push_str("\nActive managers retry pending boards automatically. You can also rerun this command.");
        }
        text
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("device_revoke")),
            ("device_id".into(), json!(self.device)),
            ("name".into(), json!(self.name)),
            ("confirmed".into(), json!(self.confirmed)),
            ("complete".into(), json!(self.complete())),
            ("boards".into(), json!(self.boards)),
            ("notices".into(), json!(self.notices)),
        ])
    }
}

use std::{collections::BTreeMap, path::Path, time::Duration};

use pepino_app::{BoardJoinState, DeviceGrant};
use pepino_core::BoardId;
use ppn::daemon::{
    address::socket_path,
    client::Client,
    process,
    protocol::{Operation, Outcome},
};

use super::*;
use crate::commands::daemon::{ensure_running, unreachable};

const PENDING: &str = "The join is kept and will complete when the granting device is reachable. Check with `ppn status` or `ppn sync`.";

pub(super) fn run(
    context: &Context<'_>,
    code: &str,
    selection: Option<&Path>,
) -> Result<CommandResult, CliError> {
    let grant: DeviceGrant = Kind::Grant.decode(code).map_err(invalid_code)?;
    let (directory, options) = local_options(context, None)?;
    let local = runtime()?.block_on(App::join_device(directory, grant, options))?;
    let config = &context.config()?.config;
    let client = Client {
        path: socket_path(config),
    };
    if config.daemon.autostart.value {
        ensure_running(&client, context, selection, true)?;
    } else {
        process::wait_for_service(&client, process::START_LIMIT).map_err(unreachable)?;
    }
    if let Some(peer) = local.peer_hint() {
        let outcome = client
            .call(Operation::RegisterPeerHint { peer })
            .map_err(unreachable)?;
        if !matches!(outcome, Outcome::PeerHintRegistered) {
            return Err(unexpected_reply());
        }
    }
    let wait = context.timeout.unwrap_or(Duration::from_secs(30));
    let timeout_ms = wait.as_millis().try_into().unwrap_or(u64::MAX);
    let outcome = client
        .call_with(
            Operation::BoardJoinState { timeout_ms },
            wait.saturating_add(Duration::from_secs(2)),
        )
        .map_err(unreachable)?;
    let Outcome::BoardJoinState { boards } = outcome else {
        return Err(unexpected_reply());
    };
    Ok(CommandResult::document(JoinResult(boards)))
}

struct JoinResult(BTreeMap<BoardId, BoardJoinState>);

impl JoinResult {
    fn joined(&self) -> bool {
        self.0
            .values()
            .all(|state| *state == BoardJoinState::Joined)
    }
}

impl Render for JoinResult {
    fn human(&self, _: &Terminal) -> String {
        if self.joined() {
            "Device joined. Initial sync complete.".into()
        } else {
            format!("Timed out waiting for initial sync. {PENDING}")
        }
    }

    fn json_fields(self) -> Map<String, Value> {
        let joined = self.joined();
        Map::from_iter([
            ("kind".into(), json!("device_join")),
            ("joined".into(), json!(joined)),
            ("boards".into(), json!(self.0.values().collect::<Vec<_>>())),
            ("message".into(), json!((!joined).then_some(PENDING))),
        ])
    }
}

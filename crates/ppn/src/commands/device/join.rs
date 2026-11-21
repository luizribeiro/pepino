use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, Instant},
};

use pepino_app::{BoardJoinState, SignedGrant};
use pepino_core::BoardId;
use ppn::daemon::{
    address::socket_path,
    client::Client,
    process,
    protocol::{Operation, Outcome, wait_millis},
};

use super::*;
use crate::commands::daemon::{ensure_running, unreachable};

const PENDING: &str = "The join is kept and will complete when the granting device is reachable. Check with `ppn status` or `ppn sync`.";

pub(super) fn run(
    context: &Context<'_>,
    code: &str,
    selection: Option<&Path>,
) -> Result<CommandResult, CliError> {
    let wait = context.timeout.unwrap_or(Duration::from_secs(30));
    wait_millis(wait)?;
    let grant: SignedGrant = Kind::Grant.decode(code).map_err(invalid_code)?;
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
    let peer = local.peer_hint().ok_or_else(unexpected_reply)?;
    let id = peer.id;
    let name = local
        .devices()
        .remove(&id)
        .unwrap_or_else(|| format!("unknown device {}", ppn::render::short_id(&id.to_string())));
    let grantor = Grantor { id, name };
    let outcome = wait_for_boards(&client, peer, wait, None)?;
    let Outcome::BoardJoinState { boards } = outcome else {
        return Err(unexpected_reply());
    };
    let grantor = pepino_app::LocalState::load(directory)
        .ok()
        .and_then(|local| local.devices().remove(&id))
        .map_or(grantor, |name| Grantor { id, name });
    let result = JoinResult { boards, grantor };
    if !result.joined() {
        return Err(CliError::new(
            ErrorKind::Pending,
            format!(
                "{}. Timed out waiting for initial sync. {PENDING}",
                result.grantor.display()
            ),
        )
        .with_detail("joined", json!(false))
        .with_detail("boards", json!(result.boards.values().collect::<Vec<_>>()))
        .with_detail("grantor", json!(result.grantor)));
    }
    Ok(CommandResult::document(result))
}

pub(crate) fn wait_for_boards(
    client: &Client,
    peer: pepino_app::PeerHint,
    wait: Duration,
    board: Option<BoardId>,
) -> Result<Outcome, CliError> {
    let start = Instant::now();
    loop {
        let remaining = wait.saturating_sub(start.elapsed());
        let outcome = client
            .call(Operation::RegisterPeerHint { peer: peer.clone() })
            .and_then(|outcome| match outcome {
                Outcome::PeerHintRegistered => client.call_with(
                    Operation::BoardJoinState {
                        board,
                        timeout_ms: remaining.as_millis().try_into().unwrap_or(0),
                    },
                    remaining + Duration::from_secs(2),
                ),
                response @ Outcome::Error { .. } => Ok(response),
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Expected a peer hint acknowledgement",
                )),
            });
        match outcome {
            Ok(Outcome::Error { message, .. }) => {
                return Err(CliError::new(ErrorKind::Failure, message));
            }
            Ok(outcome) => return Ok(outcome),
            Err(error)
                if !remaining.is_zero()
                    && (ppn::daemon::client::unavailable(&error)
                        || matches!(
                            error.kind(),
                            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::TimedOut
                        )) =>
            {
                process::wait_for_service(client, wait.saturating_sub(start.elapsed()))
                    .map_err(unreachable)?;
            }
            Err(error) => return Err(unreachable(error)),
        }
    }
}

struct JoinResult {
    boards: BTreeMap<BoardId, BoardJoinState>,
    grantor: Grantor,
}

#[derive(serde::Serialize)]
struct Grantor {
    id: DeviceId,
    name: String,
}

impl Grantor {
    fn display(&self) -> String {
        format!(
            "Granted by {} ({})",
            single_line(&self.name),
            ppn::render::short_id(&self.id.to_string())
        )
    }
}

impl JoinResult {
    fn joined(&self) -> bool {
        self.boards
            .values()
            .all(|state| *state == BoardJoinState::Joined)
    }
}

impl Render for JoinResult {
    fn human(&self, _: &Terminal) -> String {
        format!(
            "Device joined. Initial sync complete. {}.",
            self.grantor.display()
        )
    }

    fn json_fields(self) -> Map<String, Value> {
        let joined = self.joined();
        Map::from_iter([
            ("kind".into(), json!("device_join")),
            ("joined".into(), json!(joined)),
            (
                "boards".into(),
                json!(self.boards.values().collect::<Vec<_>>()),
            ),
            ("grantor".into(), json!(self.grantor)),
            ("message".into(), json!((!joined).then_some(PENDING))),
        ])
    }
}

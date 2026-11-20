use std::{path::Path, time::Duration};

use pepino_app::{App, BoardJoinState, SignedGrant};
use ppn::{
    daemon::{
        address::socket_path,
        client::Client,
        process,
        protocol::{Operation, Outcome, wait_millis},
    },
    device_code::Kind,
    error::{CliError, ErrorKind},
    output::{CommandResult, Render},
    terminal::{Context, Terminal},
};
use serde_json::{Map, Value, json};

use crate::commands::{
    daemon::{ensure_running, unreachable},
    device::{invalid_code, join::wait_for_boards, local_options, runtime, unexpected_reply},
};

pub(super) fn run(
    context: &Context<'_>,
    code: &str,
    config_path: Option<&Path>,
) -> Result<CommandResult, CliError> {
    let wait = context.timeout.unwrap_or(Duration::from_secs(30));
    wait_millis(wait)?;
    let grant: SignedGrant = Kind::BoardGrant.decode(code).map_err(invalid_code)?;
    let (directory, options) = local_options(context, None)?;
    let (board, peer, saved) = runtime()?.block_on(async {
        let app = App::open_with_options(directory, context.time.now_ms, options).await?;
        app.join_board(grant).await
    })?;
    if saved != BoardJoinState::Joined {
        let config = &context.config()?.config;
        let daemon = Client {
            path: socket_path(config),
        };
        if config.daemon.autostart.value {
            ensure_running(&daemon, context, config_path, true)?;
        } else {
            process::wait_for_service(&daemon, process::START_LIMIT).map_err(unreachable)?;
        }
        daemon.call(Operation::Nudge).map_err(unreachable)?;
        let Outcome::BoardJoinState { boards } = wait_for_boards(&daemon, peer, wait, Some(board))?
        else {
            return Err(unexpected_reply());
        };
        let state = boards.get(&board).copied().unwrap_or(saved);
        if state != BoardJoinState::Joined {
            return Err(CliError::new(ErrorKind::Pending, "Timed out waiting for initial sync. The join is saved and will resume when the granting device is reachable.")
                .with_detail("joined", json!(false)).with_detail("membership", json!(state)));
        }
    }
    Ok(CommandResult::document(Joined))
}

struct Joined;

impl Render for Joined {
    fn human(&self, _: &Terminal) -> String {
        "Board joined. Initial sync complete.".into()
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("board_join")),
            ("joined".into(), json!(true)),
        ])
    }
}

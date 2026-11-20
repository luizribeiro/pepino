use pepino_app::BoardJoinState;
use pepino_core::DeviceId;
use ppn::{
    app::AppClient,
    daemon::{address::socket_path, client::Client},
    render::{short_id, single_line},
};
use serde::Serialize;

use super::*;

#[derive(Serialize)]
struct Device {
    id: DeviceId,
    name: String,
    this_device: bool,
    state: &'static str,
    last_seen: Option<chrono::DateTime<chrono::Utc>>,
    boards: Vec<DeviceBoard>,
}

#[derive(Serialize)]
struct DeviceBoard {
    id: pepino_core::BoardId,
    name: String,
    membership: &'static str,
}

pub(super) fn run(context: &Context<'_>) -> Result<CommandResult, CliError> {
    let app = AppClient::open(context)?;
    let local = app.local_state();
    let path = socket_path(&context.config()?.config);
    let status = (Client { path }).status().ok().flatten();
    let network = status.as_ref().map(|status| &status.network);
    let complete = local.boards().iter().all(|board| {
        network
            .and_then(|network| network.board_joins.get(&board.id).copied())
            .unwrap_or_else(|| local.board_join_state(board.id))
            == BoardJoinState::Joined
    });
    let devices = local
        .devices()
        .into_iter()
        .map(|(id, name)| {
            let this_device = id == local.device_id;
            let label = id.to_string();
            let seen_on_board = local.saw_device_on_board(id)
                || network.is_some_and(|network| network.peers_synced.contains(&label));
            let boards = local
                .boards()
                .iter()
                .map(|board| {
                    let membership = if this_device {
                        match network
                            .and_then(|network| network.board_joins.get(&board.id).copied())
                            .unwrap_or_else(|| local.board_join_state(board.id))
                        {
                            BoardJoinState::Joined => "joined",
                            BoardJoinState::WaitingToBeAdded => "waiting_to_be_added",
                            _ => "waiting_for_first_sync",
                        }
                    } else {
                        match network.and_then(|network| network.board_members.get(&board.id)) {
                            Some(members) if members.contains(&id) => "member",
                            Some(_) => "pending",
                            None => "unknown",
                        }
                    };
                    DeviceBoard {
                        id: board.id,
                        name: board.name.clone(),
                        membership,
                    }
                })
                .collect();
            Device {
                id,
                name,
                this_device,
                boards,
                state: match (this_device, complete, seen_on_board) {
                    (true, false, _) => "pending",
                    (true, true, _) => "joined",
                    (false, _, true) => "seen_on_board",
                    (false, _, false) if local.added_device_here(id) => "added_not_seen",
                    (false, _, false) => "known_not_seen",
                },
                last_seen: network.and_then(|network| network.last_seen.get(&label).copied()),
            }
        })
        .collect();
    Ok(CommandResult::document(Devices(
        devices,
        context.render_settings()?,
    )))
}

struct Devices(Vec<Device>, ppn::terminal::RenderSettings);

impl Render for Devices {
    fn human(&self, terminal: &Terminal) -> String {
        self.0
            .iter()
            .map(|device| {
                let mut text = format!(
                    "{} ({}) · {}",
                    single_line(&device.name),
                    short_id(&device.id.to_string()),
                    match device.state {
                        "seen_on_board" => "seen on the board",
                        "added_not_seen" => "added here, not yet seen",
                        "known_not_seen" => "in roster, not yet seen",
                        state => state,
                    }
                );
                if device.this_device {
                    text.push_str(" · this device");
                }
                if let Some(time) = device.last_seen {
                    text.push_str(&format!(
                        " · last seen {}",
                        self.1.dates().timestamp(time, terminal)
                    ));
                }
                for board in &device.boards {
                    text.push_str(&format!(
                        "\n  {} ({}) · {}",
                        single_line(&board.name),
                        short_id(&board.id.to_string()),
                        board.membership.replace('_', " ")
                    ));
                }
                text
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("devices")),
            ("devices".into(), json!(self.0)),
        ])
    }
}

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
            let seen_on_board =
                network.is_some_and(|network| network.peers_synced.contains(&label));
            Device {
                id,
                name,
                this_device,
                state: match (this_device, complete, seen_on_board) {
                    (true, false, _) => "pending",
                    (true, true, _) => "joined",
                    (false, _, true) => "seen_on_board",
                    (false, _, false) => "added_not_seen",
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

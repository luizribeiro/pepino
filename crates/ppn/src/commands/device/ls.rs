use pepino_app::{BoardJoinState, DeviceRequest};
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
    let app = match AppClient::open(context) {
        Ok(app) => app,
        Err(error) if error.kind() == ErrorKind::NotInitialized => {
            let (directory, _) = local_options(context, None)?;
            let request = DeviceRequest::load(directory)?.ok_or(error)?;
            return Ok(CommandResult::document(Devices(vec![Device {
                id: request.id,
                name: request.name,
                this_device: true,
                state: "pending",
                last_seen: None,
            }])));
        }
        Err(error) => return Err(error),
    };
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
            Device {
                id,
                name,
                this_device,
                state: if this_device && !complete {
                    "pending"
                } else {
                    "joined"
                },
                last_seen: network
                    .and_then(|network| network.last_seen.get(&id.to_string()).copied()),
            }
        })
        .collect();
    Ok(CommandResult::document(Devices(devices)))
}

struct Devices(Vec<Device>);

impl Render for Devices {
    fn human(&self, _: &Terminal) -> String {
        self.0
            .iter()
            .map(|device| {
                let mut text = format!(
                    "{} ({}) · {}",
                    single_line(&device.name),
                    short_id(&device.id.to_string()),
                    device.state
                );
                if device.this_device {
                    text.push_str(" · this device");
                }
                if let Some(time) = device.last_seen {
                    text.push_str(&format!(" · last seen {}", time.to_rfc3339()));
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

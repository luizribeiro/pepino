use pepino_app::{BoardJoinState, RosterDevice, RosterState};
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
    roster_state: RosterState,
    member_of: usize,
    total_boards: usize,
    last_seen: Option<chrono::DateTime<chrono::Utc>>,
    boards: Vec<DeviceBoard>,
}

#[derive(Serialize)]
struct DeviceBoard {
    id: pepino_core::BoardId,
    name: String,
    membership: &'static str,
}

pub(super) fn run(context: &Context<'_>, active: bool) -> Result<CommandResult, CliError> {
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
    let roster: Vec<_> = local
        .roster()
        .into_values()
        .filter(|device| !active || device.state == RosterState::Active)
        .collect();
    let devices = roster
        .iter()
        .map(
            |RosterDevice {
                 id,
                 name,
                 state: roster_state,
                 leave_requested_at,
                 ..
             }| {
                let id = *id;
                let this_device = id == local.device_id;
                let revoked = *roster_state == RosterState::Revoked;
                let label = id.to_string();
                let seen_on_board = local.saw_device_on_board(id)
                    || network.is_some_and(|network| network.peers_synced.contains(&label));
                let boards = local
                    .boards()
                    .iter()
                    .map(|board| {
                        let membership = if this_device && !revoked {
                            match network
                                .and_then(|network| network.board_joins.get(&board.id).copied())
                                .unwrap_or_else(|| local.board_join_state(board.id))
                            {
                                BoardJoinState::Joined => "joined",
                                BoardJoinState::WaitingToBeAdded => "waiting_to_be_added",
                                _ => "waiting_for_first_sync",
                            }
                        } else {
                            match network
                                .and_then(|network| network.board_members.get(&board.id))
                                .or_else(|| local.observed_members(board.id))
                            {
                                Some(members) if members.contains(&id) => "member",
                                Some(_) if revoked => "removed",
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
                    .collect::<Vec<_>>();
                let member_of = boards
                    .iter()
                    .filter(|board| matches!(board.membership, "member" | "joined"))
                    .count();
                Device {
                    id,
                    name: name.clone(),
                    roster_state: *roster_state,
                    member_of,
                    total_boards: boards.len(),
                    this_device,
                    boards,
                    state: match (this_device, complete, seen_on_board) {
                        _ if revoked => "revoked",
                        _ if leave_requested_at.is_some() => "leave_pending",
                        (true, false, _) => "pending",
                        (true, true, _) => "joined",
                        (false, _, true) => "seen_on_board",
                        (false, _, false) if local.added_device_here(id) => "added_not_seen",
                        (false, _, false) => "known_not_seen",
                    },
                    last_seen: network
                        .and_then(|network| network.last_seen.get(&label).copied())
                        .or_else(|| local.observed_last_seen(id)),
                }
            },
        )
        .collect();
    Ok(CommandResult::document(Devices {
        devices,
        roster,
        settings: context.render_settings()?,
    }))
}

struct Devices {
    devices: Vec<Device>,
    roster: Vec<RosterDevice>,
    settings: ppn::terminal::RenderSettings,
}

impl Render for Devices {
    fn human(&self, terminal: &Terminal) -> String {
        self.devices
            .iter()
            .zip(&self.roster)
            .map(|(device, record)| {
                let mut text = format!(
                    "{} ({}) · {} · member of {} of {} boards · {}",
                    single_line(&device.name),
                    short_id(&device.id.to_string()),
                    match device.roster_state {
                        RosterState::Active => "active",
                        RosterState::Revoked => "revoked",
                    },
                    device.member_of,
                    device.total_boards,
                    match device.state {
                        "seen_on_board" => "seen on the board",
                        "added_not_seen" => "added here, not yet seen",
                        "known_not_seen" => "in roster, not yet seen",
                        "leave_pending" => "leave requested, awaiting another device",
                        "revoked" => "board access listed below",
                        state => state,
                    }
                );
                if device.this_device {
                    text.push_str(" · this device");
                }
                if let Some(added_by) = record.added_by {
                    text.push_str(&format!(" · added by {}", short_id(&added_by.to_string())));
                    if let Some(time) = record.added_at.and_then(|clock| {
                        chrono::DateTime::from_timestamp_millis(clock.wall_ms.try_into().ok()?)
                    }) {
                        text.push_str(&format!(
                            " on {}",
                            self.settings.dates().timestamp(time, terminal)
                        ));
                    }
                }
                if let Some(revocation) = &record.revocation {
                    text.push_str(&format!(
                        " · revoked by {} on {}",
                        short_id(&revocation.by.to_string()),
                        self.settings.dates().timestamp(revocation.at, terminal),
                    ));
                }
                if let Some(time) = device.last_seen {
                    text.push_str(&format!(
                        " · last seen {}",
                        self.settings.dates().timestamp(time, terminal)
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
                if device.roster_state == RosterState::Revoked && self.settings.terminal.color {
                    return format!("\x1b[2m{text}\x1b[0m");
                }
                text
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("devices")),
            ("devices".into(), json!(self.devices)),
            ("roster".into(), json!(self.roster)),
        ])
    }
}

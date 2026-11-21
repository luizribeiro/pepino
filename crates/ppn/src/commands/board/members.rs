use std::collections::BTreeMap;

use pepino_node::{MemberState, MemberStates, Node, Role};
use ppn::{
    app::AppClient,
    error::{CliError, ErrorKind},
    output::{CommandResult, Render},
    render::single_line,
    terminal::{Context, Terminal},
};
use serde_json::{Map, Value, json};

pub(super) fn run(
    context: &Context<'_>,
    reference: Option<&str>,
) -> Result<CommandResult, CliError> {
    let app = AppClient::open(context)?;
    let local = app.local_state();
    let board = match reference {
        Some(reference) => local.resolve_board(reference, "board")?,
        None => app.selected_board()?.0,
    };
    if local.waiting_for_access(board.id) {
        return Err(CliError::new(
            ErrorKind::PermissionDenied,
            format!(
                "This device is waiting for access to board {:?}.",
                single_line(&board.name)
            ),
        ));
    }
    let path = context
        .config()?
        .config
        .data_dir
        .value
        .as_path()
        .join("node.sqlite");
    let members = crate::commands::device::runtime()?
        .block_on(Node::persisted_members(&path, board.id))
        .map_err(|_| CliError::new(ErrorKind::Failure, "Could not read the board membership."))?;
    let labels = local.person_labels();
    let authors = local.author_devices(board.id);
    let mut people = Vec::new();
    for person in local.board_people(board.id) {
        let admitted = person.invited_by.map(|_| person.first_device);
        let rows = devices(person.devices(), &members, admitted);
        let role = person
            .devices()
            .filter(|device| admitted.is_none_or(|id| id == device.id))
            .filter_map(|device| role_name(members.get(&device.id)))
            .max_by_key(|role| match *role {
                "manage" => 3,
                "write" => 2,
                "read" => 1,
                _ => 0,
            });
        let inviter = person.invited_by.and_then(|device| {
            let (key, _) = authors.get(&device)?;
            Some(json!({"id": key.0, "root": key.1, "name": labels.get(key), "device_id": device}))
        });
        people.push(json!({"id": person.id, "root": person.key().1, "name": labels.get(&person.key()).map(String::as_str).unwrap_or_else(|| person.name()), "role": role, "devices": rows,
            "invited_at": person.invited_at, "invited_by": inviter, "invited_by_device_id": person.invited_by}));
    }
    if people.is_empty() {
        let roster = local.roster();
        people.push(json!({"id": local.identity_id, "root": local.own_person_key().1, "name": labels.get(&local.own_person_key()).map(String::as_str).unwrap_or_else(|| local.display_name()),
            "role": local.shared_access(board.id).map_or("manage", |access| match access.role {
                pepino_app::BoardRole::Read => "read", pepino_app::BoardRole::Write => "write", pepino_app::BoardRole::Manage => "manage",
            }),
            "devices": devices(roster.values(), &members, None), "invited_at": null, "invited_by": null}));
    }
    Ok(CommandResult::document(Members {
        board: board.name.clone(),
        people,
    }))
}

fn role_name(state: Option<&MemberState>) -> Option<&'static str> {
    match state {
        Some(MemberState::Member(Role::Read)) => Some("read"),
        Some(MemberState::Member(Role::Write)) => Some("write"),
        Some(MemberState::Member(Role::Manage)) => Some("manage"),
        _ => None,
    }
}

fn devices<'a>(
    roster: impl Iterator<Item = &'a pepino_app::RosterDevice>,
    members: &MemberStates,
    admitted: Option<pepino_core::DeviceId>,
) -> Vec<Value> {
    roster.map(|device| {
        let state = members.get(&device.id).filter(|_| admitted.is_none_or(|id| id == device.id));
        let membership = match state {
            Some(MemberState::Member(_)) => "member",
            Some(MemberState::Removed) => "removed",
            _ => "waiting_for_access",
        };
        let added_at = device.added_at.and_then(|time| {
            i64::try_from(time.wall_ms).ok().and_then(chrono::DateTime::from_timestamp_millis)
        });
        json!({
            "id": device.id, "name": device.name, "membership": membership, "role": role_name(state),
            "added_at": added_at, "revocation": device.revocation,
        })
    }).collect()
}

struct Members {
    board: String,
    people: Vec<Value>,
}

impl Render for Members {
    fn human(&self, _: &Terminal) -> String {
        let text = |value: &Value| single_line(value.as_str().unwrap_or_default());
        let mut lines = vec![format!("Members of {}", single_line(&self.board))];
        for person in &self.people {
            let role = person["role"].as_str().unwrap_or("waiting for access");
            let mut line = format!("{} ({role})", text(&person["name"]));
            if let Some(at) = person["invited_at"].as_str() {
                let by = person["invited_by"]["name"]
                    .as_str()
                    .or_else(|| person["invited_by_device_id"].as_str())
                    .unwrap_or("unknown");
                line.push_str(&format!(" · invited by {} on {at}", single_line(by)));
            }
            lines.push(line);
            for device in person["devices"].as_array().into_iter().flatten() {
                let mut row = format!(
                    "  {} · {}",
                    text(&device["name"]),
                    text(&device["membership"]).replace('_', " ")
                );
                if let Some(at) = device["added_at"].as_str() {
                    row.push_str(&format!(" · added {at}"));
                }
                if let Some(at) = device["revocation"]["at"].as_str() {
                    row.push_str(&format!(" · revoked {at}"));
                }
                lines.push(row);
            }
        }
        lines.join("\n")
    }

    fn json_fields(self) -> Map<String, Value> {
        BTreeMap::from([
            ("kind".into(), json!("board_members")),
            ("board".into(), json!(self.board)),
            ("people".into(), json!(self.people)),
        ])
        .into_iter()
        .collect()
    }
}

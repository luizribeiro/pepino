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
    if local.waiting_for_access(board.id) && local.board_departure(board.id).is_none() {
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
    let conflicts = local.membership_conflicts(board.id);
    let mut people = Vec::new();
    for person in local.board_people(board.id) {
        let roster = local.membership_devices(board.id, person.key());
        let roles: std::collections::BTreeSet<_> = roster
            .iter()
            .filter(|device| !conflicts.contains(device))
            .filter_map(|device| role_name(members.get(device)))
            .collect();
        let latest = local.latest_membership(board.id, person.key());
        let enrollment_blocked = local.membership_enrollment_blocked(board.id, person.key());
        let former = latest.is_some_and(|record| record.change.action.role().is_none())
            || (enrollment_blocked
                && roles.is_empty()
                && roster
                    .iter()
                    .any(|device| members.get(device) == Some(&MemberState::Removed)));
        let rows = devices(
            person
                .devices()
                .filter(|device| roster.contains(&device.id)),
            &members,
            former,
            &conflicts,
        );
        let role = if former {
            None
        } else if let Some(record) = latest {
            record.change.action.role().map(|role| role.to_string())
        } else {
            roster
                .get(&person.first_device)
                .filter(|id| !conflicts.contains(id))
                .and_then(|id| role_name(members.get(id)))
                .or_else(|| roles.iter().next().copied())
                .map(str::to_string)
        };
        let inviter = person.invited_by.and_then(|device| {
            let (key, _) = authors.get(&device)?;
            Some(json!({"id": key.0, "root": key.1, "name": labels.get(key), "device_id": device}))
        });
        people.push(json!({"id": person.id, "root": person.key().1, "name": labels.get(&person.key()).map(String::as_str).unwrap_or_else(|| person.name()), "role": role, "mixed_roles": roles.len() > 1, "former_member": former, "enrollment_blocked": enrollment_blocked, "devices": rows,
            "invited_at": person.invited_at, "invited_by": inviter, "invited_by_device_id": person.invited_by}));
    }
    if people.is_empty() {
        let roster = local.roster();
        people.push(json!({"id": local.identity_id, "root": local.own_person_key().1, "name": labels.get(&local.own_person_key()).map(String::as_str).unwrap_or_else(|| local.display_name()),
            "role": local.shared_access(board.id).map_or("manage", |access| match access.role {
                pepino_app::BoardRole::Read => "read", pepino_app::BoardRole::Write => "write", pepino_app::BoardRole::Manage => "manage",
            }),
            "devices": devices(roster.values(), &members, false, &conflicts), "invited_at": null, "invited_by": null}));
    }
    let reader_updates_unavailable = members.get(&local.device_id)
        == Some(&MemberState::Member(Role::Read))
        && local.board_departure(board.id).is_none()
        && !local.card_exchanges().iter().any(|(&channel, route)| {
            route.board == board.id
                && local.is_current_exchange(route, local.own_person_key())
                && crate::commands::device::runtime()
                    .ok()
                    .and_then(|runtime| {
                        runtime
                            .block_on(Node::persisted_members(&path, channel))
                            .ok()
                    })
                    .is_some_and(|channel_members| {
                        channel_members.iter().any(|(device, state)| {
                            *state == MemberState::Member(Role::Manage)
                                && members.get(device) == Some(&MemberState::Member(Role::Manage))
                        })
                    })
        });
    Ok(CommandResult::document(Members {
        board: board.name.clone(),
        people,
        history: super::history::records(local, board.id),
        departure: local.board_departure(board.id),
        reader_updates_unavailable,
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
    former: bool,
    conflicts: &std::collections::BTreeSet<pepino_core::DeviceId>,
) -> Vec<Value> {
    roster.map(|device| {
        let state = members.get(&device.id);
        let membership = match state {
            _ if conflicts.contains(&device.id) => "identity_conflict",
            Some(MemberState::Member(_)) if former || device.state == pepino_app::RosterState::Revoked => "removal_pending",
            Some(MemberState::Member(_)) => "member",
            Some(MemberState::Removed) => "removed",
            _ => "waiting_for_manager",
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
    history: Vec<Value>,
    departure: Option<pepino_app::BoardDeparture>,
    reader_updates_unavailable: bool,
}

impl Render for Members {
    fn human(&self, _: &Terminal) -> String {
        let text = |value: &Value| single_line(value.as_str().unwrap_or_default());
        let mut lines = vec![format!("Members of {}", single_line(&self.board))];
        if let Some(departure) = self.departure {
            lines.push(format!("Membership {departure}. Showing local history."));
        }
        if self.reader_updates_unavailable {
            lines.push(
                "Reader updates cannot be relayed until a board manager joins the update channel."
                    .into(),
            );
        }
        for person in &self.people {
            let role = if person["former_member"].as_bool() == Some(true) {
                "former member"
            } else {
                person["role"].as_str().unwrap_or("waiting for a manager")
            };
            let mut line = format!("{} ({role})", text(&person["name"]));
            if person["enrollment_blocked"].as_bool() == Some(true) {
                line.push_str(" · automatic enrollment paused until a manager invites again");
            }
            if person["mixed_roles"].as_bool() == Some(true) {
                line.push_str(" · device roles differ");
            }
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
        lines.extend(self.history.iter().map(super::history::line));
        lines.join("\n")
    }

    fn json_fields(self) -> Map<String, Value> {
        BTreeMap::from([
            ("kind".into(), json!("board_members")),
            ("board".into(), json!(self.board)),
            ("people".into(), json!(self.people)),
            ("membership_history".into(), json!(self.history)),
            ("departure".into(), json!(self.departure)),
            (
                "reader_updates_unavailable".into(),
                json!(self.reader_updates_unavailable),
            ),
        ])
        .into_iter()
        .collect()
    }
}

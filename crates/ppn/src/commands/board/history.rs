use pepino_app::{LocalState, MembershipAction};
use pepino_core::BoardId;
use ppn::render::{Dates, single_line, utc_seconds};
use serde_json::{Value, json};

pub(crate) fn records(local: &LocalState, board: BoardId) -> Vec<Value> {
    let people = local.person_labels();
    let devices = local.author_devices(board);
    let actor = |device| {
        let key = devices.get(&device).map(|(key, _)| *key);
        json!({"device_id": device, "id": key.map(|key| key.0), "root": key.map(|key| key.1),
            "name": key.and_then(|key| people.get(&key))})
    };
    let mut rows = Vec::new();
    for change in local.membership_history(board) {
        let key = change.change.person;
        let action = match change.change.action {
            MembershipAction::Invited(_) => "invited",
            MembershipAction::RoleChanged(_) => "role_changed",
            MembershipAction::Removed => "removed",
            MembershipAction::Left => "left",
        };
        rows.push((
            change.change.at,
            json!({
                "operation_id": change.operation, "at": utc_seconds(change.change.at), "action": action,
                "person": {"id": key.0, "root": key.1, "name": people.get(&key)},
                "actor": actor(change.actor), "role": change.change.action.role(),
                "applied": change.applied && change.refusal.is_none(),
                "not_applied_reason": change.refusal.map(|reason| reason.to_string()),
                "superseded": local.membership_superseded(board, change)
            }),
        ));
    }
    for person in local.board_people(board) {
        if !local.membership_history(board).any(|record| {
            record.change.person == person.key()
                && matches!(record.change.action, MembershipAction::Invited(_))
        }) && let (Some(at), Some(by)) = (person.invited_at, person.invited_by)
        {
            rows.push((at, json!({
                "at": utc_seconds(at), "action": "invited", "person": {"id": person.id, "root": person.key().1,
                    "name": people.get(&person.key())}, "actor": actor(by), "role": null
            })));
        }
    }
    rows.sort_by_key(|(time, _)| *time);
    rows.into_iter().map(|(_, row)| row).collect()
}

pub(crate) fn line(row: &Value, dates: Dates<'_>) -> String {
    let name = single_line(
        row["person"]["name"]
            .as_str()
            .or(row["person"]["id"].as_str())
            .unwrap_or("unknown"),
    );
    let by = single_line(
        row["actor"]["name"]
            .as_str()
            .or(row["actor"]["device_id"].as_str())
            .unwrap_or("unknown"),
    );
    let at = dates.json_timestamp(&row["at"]).unwrap_or_default();
    let role = row["role"].as_str().unwrap_or_default();
    let line = match row["action"].as_str() {
        Some("invited") if role.is_empty() => format!("{at} · {by} invited {name}"),
        Some("invited") => format!("{at} · {by} invited {name} as {role}"),
        Some("role_changed") => format!("{at} · {by} changed {name}'s role to {role}"),
        Some("removed") => format!("{at} · {by} removed {name}"),
        _ if name == by => format!("{at} · {name} left"),
        _ => format!("{at} · {name} left (processed by {by})"),
    };
    if let Some(reason) = row["not_applied_reason"].as_str() {
        format!("{line} · not applied ({reason})")
    } else if row["superseded"].as_bool() == Some(true) {
        format!("{line} · superseded")
    } else {
        line
    }
}

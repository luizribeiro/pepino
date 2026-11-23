use pepino_app::{LocalState, MembershipAction};
use pepino_core::BoardId;
use ppn::render::{Dates, single_line, utc_seconds};
use serde_json::{Value, json};

#[derive(Clone, Copy)]
enum Action {
    Invited,
    RoleChanged,
    Removed,
}

impl Action {
    fn tag(self) -> &'static str {
        match self {
            Self::Invited => "invited",
            Self::RoleChanged => "role_changed",
            Self::Removed => "removed",
        }
    }
}

#[derive(Clone, serde::Serialize)]
#[serde(transparent)]
pub(crate) struct Row {
    #[serde(skip)]
    action: Action,
    fields: Value,
}

pub(crate) fn records(local: &LocalState, board: BoardId) -> Vec<Row> {
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
            MembershipAction::Invited(_) => Action::Invited,
            MembershipAction::RoleChanged(_) => Action::RoleChanged,
            MembershipAction::Removed => Action::Removed,
        };
        rows.push((
            change.change.at,
            Row { action, fields: json!({
                "operation_id": change.operation, "at": utc_seconds(change.change.at), "action": action.tag(),
                "person": {"id": key.0, "root": key.1, "name": people.get(&key)},
                "actor": actor(change.actor), "role": change.change.action.role(),
                "applied": change.applied && change.refusal.is_none(),
                "not_applied_reason": change.refusal.map(|reason| reason.to_string()),
                "superseded": local.membership_superseded(board, change)
            }) },
        ));
    }
    for person in local.board_people(board) {
        if !local.membership_history(board).any(|record| {
            record.change.person == person.key()
                && matches!(record.change.action, MembershipAction::Invited(_))
        }) && let (Some(at), Some(by)) = (person.invited_at, person.invited_by)
        {
            let action = Action::Invited;
            rows.push((at, Row { action, fields: json!({
                "at": utc_seconds(at), "action": action.tag(), "person": {"id": person.id, "root": person.key().1,
                    "name": people.get(&person.key())}, "actor": actor(by), "role": null
            }) }));
        }
    }
    rows.sort_by_key(|(time, _)| *time);
    rows.into_iter().map(|(_, row)| row).collect()
}

pub(crate) fn line(record: &Row, dates: Dates<'_>) -> String {
    let row = &record.fields;
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
    let line = match record.action {
        Action::Invited if role.is_empty() => format!("{at} · {by} invited {name}"),
        Action::Invited => format!("{at} · {by} invited {name} as {role}"),
        Action::RoleChanged => format!("{at} · {by} changed {name}'s role to {role}"),
        Action::Removed => format!("{at} · {by} removed {name}"),
    };
    if let Some(reason) = row["not_applied_reason"].as_str() {
        format!("{line} · not applied ({reason})")
    } else if row["superseded"].as_bool() == Some(true) {
        format!("{line} · superseded")
    } else {
        line
    }
}

#[test]
fn renders_each_membership_action_with_its_json_tag() {
    let dates = Dates {
        time: ppn::app::InvocationTime {
            now_ms: 0,
            today: "1970-01-01".parse().unwrap(),
            offset: chrono::FixedOffset::east_opt(0).unwrap(),
        },
        format: "%Y-%m-%d",
    };
    for (action, role, expected) in [
        (Action::Invited, None, "Ada invited Ben"),
        (Action::Invited, Some("write"), "Ada invited Ben as write"),
        (
            Action::RoleChanged,
            Some("read"),
            "Ada changed Ben's role to read",
        ),
        (Action::Removed, None, "Ada removed Ben"),
    ] {
        let fields = json!({
            "action": action.tag(), "role": role,
            "person": {"name": "Ben"}, "actor": {"name": "Ada"}
        });
        let row = Row {
            action,
            fields: fields.clone(),
        };
        assert!(line(&row, dates).ends_with(expected));
        assert_eq!(json!(row), fields);
    }
}

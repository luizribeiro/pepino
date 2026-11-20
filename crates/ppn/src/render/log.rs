use std::collections::BTreeMap;

use pepino_app::{EventClassification, EventRecord};
use ppn::render::{Dates, assignee_label, single_line};
use ppn::response::Log;

pub(super) fn history(log: Log<'_>, dates: Dates<'_>) -> String {
    let (applied, mut skipped): (Vec<_>, Vec<_>) = log
        .events
        .iter()
        .partition(|event| event.classification == EventClassification::Applied);
    skipped.extend(log.unordered);
    let mut rows: Vec<_> = applied
        .into_iter()
        .map(|event| event_row(event, log.names, dates))
        .collect();
    if !skipped.is_empty() {
        rows.push("Unordered or skipped events".into());
        rows.extend(skipped.into_iter().map(|event| {
            let row = event_row(event, log.names, dates);
            format!("{row} [{}]", classification(event.classification))
        }));
    }
    rows.join("\n")
}

fn event_row(event: &EventRecord, names: &BTreeMap<String, String>, dates: Dates<'_>) -> String {
    let date = event.hlc.map_or_else(
        || "unknown time".into(),
        |clock| dates.local_date_time(clock),
    );
    let author = event
        .author
        .as_deref()
        .map_or_else(|| "unknown author".into(), |id| assignee_label(id, names));
    let summary = single_line(&event.change.summary);
    format!("{date}  {author}  {summary}")
}

fn classification(classification: EventClassification) -> &'static str {
    match classification {
        EventClassification::Applied => "applied",
        EventClassification::UnknownVersion => "unknown_version",
        EventClassification::UnknownEvent => "unknown_event",
        EventClassification::UnknownValue => "unknown_value",
        EventClassification::Invalid => "invalid",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::history;
    use pepino_app::Response;
    use ppn::{app::InvocationTime, render::Dates, response::ResponseExt};
    use serde_json::json;

    #[test]
    fn history_moves_skipped_and_unordered_records_to_a_trailing_section() {
        let timestamp =
            json!({"wall_ms":1_795_149_900_000_u64,"counter":1,"utc":"2026-11-20T04:45:00.000Z"});
        let response: Response = serde_json::from_value(json!({
            "kind": "log", "titles": {}, "names": {"678abc": "Garden laptop"},
            "events": [
                {"id":"100abc","author":"678abc","hlc":timestamp,"classification":"unknown_event",
                 "kind":"reminder_added","change":{"type":"skipped","tasks":[],"summary":"Skipped event"}},
                {"id":"101abc","author":"678abc","hlc":timestamp,"classification":"applied",
                 "kind":"tag_added","change":{"type":"tag","task":"741abc","tag":"hardware","summary":"Added tag hardware"}}
            ],
            "unordered": [
                {"id":"103abc","author":null,"hlc":null,"classification":"unknown_version",
                 "kind":"unknown","change":{"type":"skipped","tasks":[],"summary":"Skipped event"}}
            ]
        })).unwrap();
        let dates = Dates {
            time: InvocationTime {
                today: "2026-11-19".parse().unwrap(),
                now_ms: 1_795_149_900_000,
                offset: chrono::FixedOffset::west_opt(18_000).unwrap(),
            },
            format: "%d %b %Y",
        };
        assert_eq!(
            history(response.log().unwrap(), dates),
            "19 Nov 2026 23:45:00  Garden laptop  Added tag hardware\nUnordered or skipped events\n19 Nov 2026 23:45:00  Garden laptop  Skipped event [unknown_event]\nunknown time  unknown author  Skipped event [unknown_version]"
        );
    }
}

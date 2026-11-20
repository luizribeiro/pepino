use pepino_app::{EventClassification, EventDetail, EventRecord};
use ppn::render::{Dates, ShortIds};
use ppn::response::Log;

pub(super) fn history(
    log: Log<'_>,
    dates: Dates<'_>,
    author: impl Fn(Option<&str>) -> String,
) -> String {
    let (applied, mut skipped): (Vec<_>, Vec<_>) = log
        .events
        .iter()
        .partition(|event| event.classification == EventClassification::Applied);
    skipped.extend(log.unordered);
    let ids = ShortIds::new(
        log.titles.keys().map(String::as_str).chain(
            log.events
                .iter()
                .chain(log.unordered)
                .filter_map(|event| match &event.change.detail {
                    EventDetail::Assignee { who, .. } => Some(who.as_str()),
                    _ => None,
                }),
        ),
    );
    let row = |event| event_row(event, &log, dates, &ids, &author);
    let mut rows: Vec<_> = applied.into_iter().map(row).collect();
    if !skipped.is_empty() {
        rows.push("Unordered or skipped events".into());
        rows.extend(skipped.into_iter().map(|event| {
            let row = row(event);
            format!("{row} [{}]", classification(event.classification))
        }));
    }
    rows.join("\n")
}

fn event_row(
    event: &EventRecord,
    log: &Log<'_>,
    dates: Dates<'_>,
    ids: &ShortIds<'_>,
    author: &impl Fn(Option<&str>) -> String,
) -> String {
    let date = event.hlc.map_or_else(
        || "unknown time".into(),
        |clock| dates.local_date_time(clock),
    );
    let author = author(event.author.as_deref());
    let summary = event
        .change
        .summary_with(log.titles, log.names, |id| ids.get(id).to_owned());
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
            "kind": "log", "titles": {"741abc": "Repair the latch"}, "names": {"678abc": "Garden laptop"},
            "events": [
                {"id":"100abc","author":"678abc","hlc":timestamp,"classification":"unknown_event",
                 "kind":"reminder_added","change":{"type":"skipped","tasks":[],"summary":"Skipped event"}},
                {"id":"101abc","author":"678abc","hlc":timestamp,"classification":"applied",
                 "kind":"tag_added","change":{"type":"tag","task":"741abc","action":"added","tag":"hardware","summary":"Added tag hardware"}}
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
            history(response.log().unwrap(), dates, |id| if id.is_some() {
                "Ada".into()
            } else {
                "unknown person".into()
            }),
            "19 Nov 2026 23:45  Ada  Repair the latch: tag #hardware added\nUnordered or skipped events\n19 Nov 2026 23:45  Ada  Skipped event [unknown_event]\nunknown time  unknown person  Skipped event [unknown_version]"
        );
    }
}

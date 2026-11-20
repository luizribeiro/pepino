use ciborium::Value;
use pepino_core::{Event, TaskId};

pub(super) fn skipped(bytes: &[u8]) -> (Option<Event>, Vec<TaskId>) {
    let Ok((_, content)) = ciborium::from_reader::<(u64, Value), _>(bytes) else {
        return (None, Vec::new());
    };
    let Some(payload) = member(&content, "event") else {
        return (None, Vec::new());
    };
    let event = payload.deserialized::<Event>().ok();
    let tasks = match &event {
        Some(event) => event.referenced_tasks().collect(),
        None => payload
            .as_map()
            .and_then(|entries| entries.first())
            .map_or_else(Vec::new, |(kind, fields)| {
                let keys: &[_] = match kind.as_text() {
                    Some("edge_added" | "edge_removed") => &["from", "to"],
                    _ => &["task"],
                };
                keys.iter()
                    .filter_map(|key| member(fields, key)?.deserialized::<TaskId>().ok())
                    .collect()
            }),
    };
    (event, tasks)
}

fn member<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    let mut matches = value
        .as_map()?
        .iter()
        .filter(|(key, _)| key.as_text() == Some(name));
    let (_, value) = matches.next()?;
    matches.next().is_none().then_some(value)
}

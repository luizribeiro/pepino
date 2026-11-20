use ciborium::Value;
use pepino_core::*;

fn map(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}

const AUTHOR: DeviceId = DeviceId::from_bytes([19; 32]);
const HLC: Hlc = Hlc {
    wall_ms: 51,
    counter: 5,
};

fn encode(variant: &'static str, payload: Value) -> Vec<u8> {
    let content = map([
        ("author", Value::serialized(&AUTHOR).unwrap()),
        ("hlc", Value::serialized(&HLC).unwrap()),
        ("event", map([(variant, payload)])),
    ]);
    let mut bytes = Vec::new();
    ciborium::into_writer(&(1, content), &mut bytes).unwrap();
    bytes
}

#[test]
fn distinguishes_unknown_nested_values_from_corruption() {
    let task = Value::serialized(&TaskId(EventId::from_bytes([59; 32]))).unwrap();
    let field = || [("task", task.clone()), ("field", "review".into())];

    let mut fields = Value::serialized(&InitialFields::default()).unwrap();
    for (key, value) in fields.as_map_mut().unwrap() {
        match key.as_text() {
            Some("status") => *value = "paused".into(),
            Some("custom") => *value = map([("review", "review_later".into())]),
            _ => {}
        }
    }
    let created = map([("title", "Repair the gate".into()), ("fields", fields)]);
    let nested_status = map(field()
        .into_iter()
        .chain([("value", map([("status", "paused".into())]))]));
    let future_value = map(field()
        .into_iter()
        .chain([("value", map([("duration", 20.into())]))]));
    let future_title = map([
        ("task", task.clone()),
        ("field", "title".into()),
        ("value", map([("rich_text", "Repair the gate".into())])),
    ]);
    let edge = |kind, to| map([("from", task.clone()), ("kind", kind), ("to", to)]);
    let to = Value::serialized(&TaskId(EventId::from_bytes([61; 32]))).unwrap();

    for (variant, payload, values) in [
        (
            "task_created",
            created,
            vec![
                UnknownValue::Status("paused".into()),
                UnknownValue::FieldValue("review_later".into()),
            ],
        ),
        (
            "task_field_set",
            nested_status,
            vec![UnknownValue::Status("paused".into())],
        ),
        (
            "task_field_set",
            future_value,
            vec![UnknownValue::FieldValue("duration".into())],
        ),
        (
            "task_field_set",
            future_title,
            vec![UnknownValue::FieldValue("rich_text".into())],
        ),
        (
            "edge_added",
            edge("depends_on".into(), to.clone()),
            vec![UnknownValue::EdgeKind("depends_on".into())],
        ),
        (
            "edge_removed",
            edge(map([("depends_on", "weather".into())]), to.clone()),
            vec![UnknownValue::EdgeKind("depends_on".into())],
        ),
    ] {
        let bytes = encode(variant, payload);
        assert_eq!(
            Envelope::decode(&bytes).unwrap(),
            DecodeOutcome::UnknownValue {
                id: EventId::from_bytes(*blake3::hash(&bytes).as_bytes()),
                author: AUTHOR,
                hlc: HLC,
                variant: variant.into(),
                values,
                bytes,
            }
        );
    }
    let bad_value = map(field()
        .into_iter()
        .chain([("value", map([("integer", "high".into())]))]));
    for (variant, payload) in [
        ("edge_added", edge("depends_on".into(), 4.into())),
        ("task_field_set", bad_value),
        (
            "edge_removed",
            edge(map([("relates", "weather".into())]), to),
        ),
    ] {
        assert!(matches!(
            Envelope::decode(&encode(variant, payload)),
            Err(DecodeError::InvalidContent)
        ));
    }
}

fn field_member<'a>(entries: &'a mut [(Value, Value)], name: &str) -> &'a mut Value {
    &mut entries
        .iter_mut()
        .find(|(key, _)| key.as_text() == Some(name))
        .unwrap()
        .1
}

fn creation(status: &str, title: &str, change: impl Fn(&mut Vec<(Value, Value)>)) -> Vec<u8> {
    let mut fields = Value::serialized(&InitialFields::default()).unwrap();
    let entries = fields.as_map_mut().unwrap();
    *field_member(entries, "status") = status.into();
    change(entries);
    encode(
        "task_created",
        map([("title", title.into()), ("fields", fields)]),
    )
}

#[test]
fn unknown_status_does_not_hide_extra_fields() {
    for status in ["todo", "paused"] {
        let bytes = creation(status, "Repair the gate", |fields| {
            fields.push(("extra".into(), true.into()));
        });
        assert!(matches!(
            Envelope::decode(&bytes),
            Err(DecodeError::NonCanonical)
        ));
    }
}

#[test]
fn unknown_status_does_not_hide_unsorted_tags() {
    for status in ["todo", "paused"] {
        let bytes = creation(status, "Repair the gate", |fields| {
            *field_member(fields, "tags") = Value::Array(vec!["repairs".into(), "garden".into()]);
        });
        assert!(matches!(
            Envelope::decode(&bytes),
            Err(DecodeError::NonCanonical)
        ));
    }
}

fn assert_invalid(bytes: &[u8], reason: InvalidEvent) {
    assert_eq!(
        Envelope::decode(bytes).unwrap(),
        DecodeOutcome::InvalidEvent {
            id: EventId::from_bytes(*blake3::hash(bytes).as_bytes()),
            author: AUTHOR,
            hlc: HLC,
            reason,
        }
    );
}

#[test]
fn unknown_status_does_not_hide_invalid_tags() {
    for status in ["todo", "paused"] {
        let bytes = creation(status, "Repair the gate", |fields| {
            *field_member(fields, "tags") = Value::Array(vec!["B".into()]);
        });
        assert_invalid(&bytes, InvalidEvent::Tag("B".into()));
    }
}

#[test]
fn unknown_status_does_not_hide_invalid_custom_names() {
    for status in ["todo", "paused"] {
        let bytes = creation(status, "Repair the gate", |fields| {
            *field_member(fields, "custom") = map([("Bad", "unset".into())]);
        });
        assert_invalid(
            &bytes,
            InvalidEvent::FieldName {
                field: "Bad".into(),
                reason: InvalidFieldName::Syntax,
            },
        );
    }
}

#[test]
fn unknown_status_does_not_hide_an_empty_title() {
    for status in ["todo", "paused"] {
        assert_invalid(&creation(status, "", |_| {}), InvalidEvent::EmptyTitle);
    }
}

#[test]
fn checks_wire_encoding_around_unknown_values() {
    let bytes = creation("paused", "Repair the gate", |_| {});
    let overlong_version = [&[0x82, 0x18, 1], &bytes[2..]].concat();
    assert!(matches!(
        Envelope::decode(&overlong_version),
        Err(DecodeError::NonCanonical)
    ));

    let label = bytes
        .windows(7)
        .position(|window| window == b"\x66paused")
        .unwrap();
    let opaque = [&bytes[..label], &[0x78, 6], &bytes[label + 1..]].concat();
    assert!(
        matches!(Envelope::decode(&opaque), Ok(DecodeOutcome::UnknownValue { bytes, .. }) if bytes == opaque)
    );
}

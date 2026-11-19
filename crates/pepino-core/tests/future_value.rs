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

#[test]
fn distinguishes_unknown_nested_values_from_corruption() {
    let author = DeviceId::from_bytes([19; 32]);
    let hlc = Hlc {
        wall_ms: 51,
        counter: 5,
    };
    let task = Value::serialized(&TaskId(EventId::from_bytes([59; 32]))).unwrap();
    let field = || [("task", task.clone()), ("field", "review".into())];
    let encode = |variant, payload| {
        let content = map([
            ("author", Value::serialized(&author).unwrap()),
            ("hlc", Value::serialized(&hlc).unwrap()),
            ("event", map([(variant, payload)])),
        ]);
        let mut bytes = Vec::new();
        // An overlong encoding of the version exercises preservation of raw bytes.
        bytes.extend_from_slice(&[0x82, 0x18, 0x01]);
        ciborium::into_writer(&content, &mut bytes).unwrap();
        bytes
    };

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
    let edge = |to| {
        map([
            ("from", task.clone()),
            ("kind", "depends_on".into()),
            ("to", to),
        ])
    };
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
            "edge_added",
            edge(to.clone()),
            vec![UnknownValue::EdgeKind("depends_on".into())],
        ),
        (
            "edge_removed",
            edge(to),
            vec![UnknownValue::EdgeKind("depends_on".into())],
        ),
    ] {
        let bytes = encode(variant, payload);
        assert_eq!(
            Envelope::decode(&bytes).unwrap(),
            DecodeOutcome::UnknownValue {
                id: EventId::from_bytes(*blake3::hash(&bytes).as_bytes()),
                author,
                hlc,
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
        ("edge_added", edge(4.into())),
        ("task_field_set", bad_value),
    ] {
        assert!(matches!(
            Envelope::decode(&encode(variant, payload)),
            Err(DecodeError::InvalidContent)
        ));
    }
}

use pepino_core::*;

#[test]
fn version_one_event_encodings_are_stable() {
    let author = DeviceId::from_bytes([0x1f; 32]);
    let hlc = Hlc {
        wall_ms: 1_795_144_800_000,
        counter: 257,
    };
    let created = Event::TaskCreated {
        title: "Repair the garden gate".into(),
        fields: InitialFields {
            description: Some("Replace the lower hinge".into()),
            status: Status::Doing,
            due: Date::from_ymd_opt(2026, 12, 1),
            priority: Some(-25),
            tags: ["garden".into(), "repairs".into()].into(),
            assignees: [IdentityId::from_bytes([0x2a; 32])].into(),
            custom: [(
                FieldName::new("estimate").unwrap(),
                FieldValue::Integer(256),
            )]
            .into(),
        },
    };
    let changed = Event::TaskFieldSet {
        task: TaskId(EventId::from_bytes([0x3b; 32])),
        field: FieldName::new("status").unwrap(),
        value: FieldValue::Status(Status::Done),
    };

    let frame_prefix = concat!(
        "8201a366617574686f725820",
        "1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f",
        "63686c63a26777616c6c5f6d731b000001a1f6f7d70067636f756e746572190101656576656e74",
    );
    for (event, payload, id) in [
        (
            created,
            concat!(
                "a16c7461736b5f63726561746564a2657469746c6576526570616972207468652067617264656e2067617465",
                "666669656c6473a76b6465736372697074696f6e775265706c61636520746865206c6f7765722068696e6765",
                "6673746174757365646f696e67636475656a323032362d31322d3031687072696f726974793818",
                "6474616773826667617264656e67726570616972736961737369676e656573815820",
                "2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a",
                "66637573746f6da168657374696d617465a167696e7465676572190100",
            ),
            "e4424bbd72eed4589d4f0ff742e5d1b0c59ddd15a97ae8bab7c1ca304bf0c6fe",
        ),
        (
            changed,
            concat!(
                "a16e7461736b5f6669656c645f736574a3647461736b5820",
                "3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b",
                "656669656c64667374617475736576616c7565a16673746174757364646f6e65",
            ),
            "77c3f41822508b8486376b11298f81409b041a74baf1ef0969eb2dc60395d1ca",
        ),
    ] {
        let envelope = Envelope::new(author, hlc, event).unwrap();
        let expected = hex::decode([frame_prefix, payload].concat()).unwrap();
        assert_eq!(envelope.encode().unwrap(), expected);
        assert_eq!(envelope.id, id.parse().unwrap());
        assert_eq!(
            Envelope::decode(&expected).unwrap(),
            DecodeOutcome::Known(envelope)
        );
    }
}

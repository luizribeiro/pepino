use pepino_core::*;

#[test]
fn version_one_event_encodings_are_stable() {
    let author = DeviceId::from_bytes([0x1f; 32]);
    let hlc = Hlc {
        wall_ms: 1_795_144_800_000,
        counter: 257,
    };
    let task = TaskId::from_bytes([0x3b; 32]);
    let to = TaskId::from_bytes([0x4c; 32]);
    let who = IdentityId::from_bytes([0x2a; 32]);
    let created = Event::TaskCreated {
        title: "Repair the garden gate".into(),
        fields: InitialFields {
            description: Some("Replace the lower hinge".into()),
            status: Status::Doing,
            due: Date::from_ymd_opt(2026, 12, 1),
            priority: Some(-25),
            tags: ["garden".into(), "repairs".into()].into(),
            assignees: [who].into(),
            custom: [
                ("approval", FieldValue::Status(Status::Cancelled)),
                ("estimate", FieldValue::Integer(256)),
                (
                    "inspection",
                    FieldValue::Date(Date::from_ymd_opt(2026, 12, 3).unwrap()),
                ),
                ("notes", FieldValue::Text("Use brass".into())),
                ("supplier", FieldValue::Unset),
                ("workshop", FieldValue::Status(Status::Todo)),
            ]
            .into_iter()
            .map(|(name, value)| (FieldName::new(name).unwrap(), value))
            .collect(),
        },
    };
    let changed = Event::TaskFieldSet {
        task,
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
                "a16c7461736b5f63726561746564a2657469746c6576526570616972207468652067617264656e20",
                "67617465666669656c6473a76b6465736372697074696f6e775265706c61636520746865206c6f77",
                "65722068696e67656673746174757365646f696e67636475656a323032362d31322d303168707269",
                "6f7269747938186474616773826667617264656e67726570616972736961737369676e6565738158",
                "202a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a66637573746f6d",
                "a668617070726f76616ca1667374617475736963616e63656c6c656468657374696d617465a16769",
                "6e74656765721901006a696e7370656374696f6ea164646174656a323032362d31322d3033656e6f",
                "746573a164746578746955736520627261737368737570706c69657265756e73657468776f726b73",
                "686f70a16673746174757364746f646f",
            ),
            "6ae9cf019d1a52deab25f90f91dc9ae8a5aaa3d923f70bcb39ed0bb1b1febd0e",
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
        (
            Event::TagAdded {
                task,
                tag: "garden".into(),
            },
            concat!(
                "a1697461675f6164646564a2647461736b58203b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b",
                "3b3b3b3b3b3b3b3b3b3b3b637461676667617264656e",
            ),
            "f27f7652af0c8513e648518020dc3c1a7c9a9eb9a4b0f56ee8e2fbb9d74aaf43",
        ),
        (
            Event::TagRemoved {
                task,
                tag: "repairs".into(),
            },
            concat!(
                "a16b7461675f72656d6f766564a2647461736b58203b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b",
                "3b3b3b3b3b3b3b3b3b3b3b3b3b637461676772657061697273",
            ),
            "062a2194b2c9021056422e7d1379bdd87166037514708d1e99e8f32ce052ad68",
        ),
        (
            Event::AssigneeAdded { task, who },
            concat!(
                "a16e61737369676e65655f6164646564a2647461736b58203b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b",
                "3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b6377686f58202a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a",
                "2a2a2a2a2a2a2a2a2a2a2a2a2a2a",
            ),
            "5cfd0ca453668ef2e5fe721d33bd763e176856afd268aae62a370aca19b6becb",
        ),
        (
            Event::AssigneeRemoved { task, who },
            concat!(
                "a17061737369676e65655f72656d6f766564a2647461736b58203b3b3b3b3b3b3b3b3b3b3b3b3b3b",
                "3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b6377686f58202a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a",
                "2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a",
            ),
            "022ba11be56eea8e6425540abe0b961d1b08a6037d608128c355baf2d2477bed",
        ),
        (
            Event::EdgeAdded {
                from: task,
                kind: EdgeKind::Blocks,
                to,
            },
            concat!(
                "a16a656467655f6164646564a36466726f6d58203b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b",
                "3b3b3b3b3b3b3b3b3b3b3b3b646b696e6466626c6f636b7362746f58204c4c4c4c4c4c4c4c4c4c4c",
                "4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c",
            ),
            "12e7faa517e3c2e4cbe2c5392ebff005e0745dac171cd4e396d1bd7047da631b",
        ),
        (
            Event::EdgeAdded {
                from: task,
                kind: EdgeKind::Relates,
                to,
            },
            concat!(
                "a16a656467655f6164646564a36466726f6d58203b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b",
                "3b3b3b3b3b3b3b3b3b3b3b3b646b696e646772656c6174657362746f58204c4c4c4c4c4c4c4c4c4c",
                "4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c",
            ),
            "c17de1f2aabf44f63502da64988e45401eaca7789e39abc321158a175f49339d",
        ),
        (
            Event::EdgeRemoved {
                from: task,
                kind: EdgeKind::Parent,
                to,
            },
            concat!(
                "a16c656467655f72656d6f766564a36466726f6d58203b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b",
                "3b3b3b3b3b3b3b3b3b3b3b3b3b3b646b696e6466706172656e7462746f58204c4c4c4c4c4c4c4c4c",
                "4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c",
            ),
            "805ce16a5714fae7b09f54347fa03da33c1f99d7730fd2bc476722f26b1f88f5",
        ),
        (
            Event::CommentAdded {
                task,
                text: "Pick up hinges".into(),
            },
            concat!(
                "a16d636f6d6d656e745f6164646564a2647461736b58203b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b",
                "3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b64746578746e5069636b2075702068696e676573",
            ),
            "544776656a6c6ac612bbfb15cdcdc443692f63a20d89b22e58e4e34467468d0d",
        ),
        (
            Event::TaskDeleted { task },
            concat!(
                "a16c7461736b5f64656c65746564a1647461736b58203b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b",
                "3b3b3b3b3b3b3b3b3b3b3b3b3b3b",
            ),
            "7c225f3ec7e10a6c0c79360c2017f8168491a84ab4b4df7fd1c451d635528e92",
        ),
    ] {
        let envelope = Envelope::new(author, hlc, event).unwrap();
        let expected = hex::decode([frame_prefix, payload].concat()).unwrap();
        assert_eq!(envelope.encode().unwrap(), expected);
        assert_eq!(envelope.id(), id.parse().unwrap());
        assert_eq!(
            Envelope::decode(&expected).unwrap(),
            DecodeOutcome::Known(envelope)
        );
    }
}

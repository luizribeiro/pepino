use pepino_core::*;

#[test]
fn field_updates_enforce_types_and_required_values() {
    let task = TaskId(EventId::from_bytes([35; 32]));
    let values = [
        FieldValue::Text("Repair the garden gate".into()),
        FieldValue::Integer(-2),
        FieldValue::Date(Date::from_ymd_opt(2026, 12, 1).unwrap()),
        FieldValue::Status(Status::Doing),
        FieldValue::Unset,
        FieldValue::Text(String::new()),
    ];
    for (name, accepted) in [
        ("title", &[0][..]),
        ("description", &[0, 4, 5]),
        ("status", &[3]),
        ("due", &[2, 4]),
        ("priority", &[1, 4]),
        ("estimate", &[0, 1, 2, 3, 4, 5]),
    ] {
        let field = FieldName::new(name).unwrap();
        for (index, value) in values.iter().enumerate() {
            let event = Event::TaskFieldSet {
                task,
                field: field.clone(),
                value: value.clone(),
            };
            let expected = if accepted.contains(&index) {
                Ok(())
            } else if name == "title" && index == 5 {
                Err(InvalidEvent::EmptyTitle)
            } else {
                Err(InvalidEvent::FieldType(field.clone()))
            };
            assert_eq!(event.validate(), expected);
        }
    }
}

#[test]
fn creation_and_updates_share_domain_rules() {
    let task = TaskId(EventId::from_bytes([35; 32]));
    for (tag, valid) in [
        ("garden/tools.repair_2-done", true),
        ("tags", true),
        ("", false),
        ("/garden", false),
        ("garden Tools", false),
        ("gardén", false),
    ] {
        let expected = if valid {
            Ok(())
        } else {
            Err(InvalidEvent::Tag(tag.into()))
        };
        let fields = InitialFields {
            tags: [tag.into()].into(),
            ..Default::default()
        };
        for event in [
            Event::TaskCreated {
                title: "Repair the garden gate".into(),
                fields,
            },
            Event::TagAdded {
                task,
                tag: tag.into(),
            },
            Event::TagRemoved {
                task,
                tag: tag.into(),
            },
        ] {
            assert_eq!(event.validate(), expected);
        }
    }
    let title = FieldName::new("title").unwrap();
    let fields = InitialFields {
        custom: [(title.clone(), FieldValue::Unset)].into(),
        ..Default::default()
    };
    let created = Event::TaskCreated {
        title: "Repair the garden gate".into(),
        fields,
    };
    assert_eq!(
        created.validate(),
        Err(InvalidEvent::BuiltInCustomField(title))
    );
    for event in [
        Event::EdgeAdded {
            from: task,
            kind: EdgeKind::Blocks,
            to: task,
        },
        Event::EdgeRemoved {
            from: task,
            kind: EdgeKind::Relates,
            to: task,
        },
    ] {
        assert_eq!(event.validate(), Err(InvalidEvent::SelfEdge(task)));
    }
}

#[test]
fn rejects_local_invalid_events_and_preserves_invalid_wire_metadata() {
    let author = DeviceId::from_bytes([74; 32]);
    let hlc = Hlc {
        wall_ms: 397,
        counter: 2,
    };
    let event = Event::TaskCreated {
        title: String::new(),
        fields: InitialFields::default(),
    };
    assert!(matches!(
        Envelope::new(author, hlc, event.clone()),
        Err(NewEnvelopeError::InvalidEvent(InvalidEvent::EmptyTitle))
    ));
    let bytes = encode_content(author, hlc, &event).unwrap();
    let invalid = |id| DecodeOutcome::InvalidEvent {
        id,
        author,
        hlc,
        reason: InvalidEvent::EmptyTitle,
    };
    assert_eq!(
        Envelope::decode(&bytes).unwrap(),
        invalid(EventId::for_content(author, hlc, &event).unwrap())
    );
    let external = EventId::from_bytes([81; 32]);
    assert_eq!(
        Envelope::decode_with_id(external, &bytes).unwrap(),
        invalid(external)
    );
    let overlong_version = [&[0x82, 0x18, 1], &bytes[2..]].concat();
    assert!(matches!(
        Envelope::decode(&overlong_version),
        Err(DecodeError::NonCanonical)
    ));
}

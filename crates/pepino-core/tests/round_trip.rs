use pepino_core::*;
use proptest::prelude::*;

fn nonempty_text() -> impl Strategy<Value = String> {
    any::<String>().prop_filter("non-empty title", |text| !text.is_empty())
}

fn status() -> impl Strategy<Value = Status> {
    prop::sample::select(vec![
        Status::Todo,
        Status::Doing,
        Status::Done,
        Status::Cancelled,
    ])
}

fn date() -> impl Strategy<Value = Date> {
    (Date::MIN.to_epoch_days()..=Date::MAX.to_epoch_days())
        .prop_map(|days| Date::from_epoch_days(days).unwrap())
}

fn field_value() -> impl Strategy<Value = FieldValue> {
    prop_oneof![
        any::<String>().prop_map(FieldValue::Text),
        any::<i64>().prop_map(FieldValue::Integer),
        date().prop_map(FieldValue::Date),
        status().prop_map(FieldValue::Status),
        Just(FieldValue::Unset),
    ]
}

fn tag() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_./-]{0,80}"
}

fn custom_name() -> impl Strategy<Value = FieldName> {
    "[a-z][a-z0-9_-]{0,63}".prop_filter_map("custom field name", |name| {
        FieldName::new(name).ok().filter(FieldName::is_custom)
    })
}

fn identity() -> impl Strategy<Value = IdentityId> {
    any::<[u8; 32]>().prop_map(IdentityId::from_bytes)
}

fn initial_fields() -> impl Strategy<Value = InitialFields> {
    (
        prop::option::of(any::<String>()),
        status(),
        prop::option::of(date()),
        prop::option::of(any::<i64>()),
        prop::collection::btree_set(tag(), 0..4),
        prop::collection::btree_set(identity(), 0..4),
        prop::collection::btree_map(custom_name(), field_value(), 0..4),
    )
        .prop_map(
            |(description, status, due, priority, tags, assignees, custom)| InitialFields {
                description,
                status,
                due,
                priority,
                tags,
                assignees,
                custom,
            },
        )
}

fn settable_field() -> impl Strategy<Value = (FieldName, FieldValue)> {
    let builtin = prop_oneof![
        nonempty_text().prop_map(|title| ("title", FieldValue::Text(title))),
        any::<String>().prop_map(|text| ("description", FieldValue::Text(text))),
        status().prop_map(|status| ("status", FieldValue::Status(status))),
        date().prop_map(|date| ("due", FieldValue::Date(date))),
        any::<i64>().prop_map(|priority| ("priority", FieldValue::Integer(priority))),
        prop::sample::select(vec!["description", "due", "priority"])
            .prop_map(|name| (name, FieldValue::Unset)),
    ]
    .prop_map(|(name, value)| (FieldName::new(name).unwrap(), value));
    prop_oneof![builtin, (custom_name(), field_value())]
}

fn events() -> impl Strategy<Value = [Event; 10]> {
    (
        nonempty_text(),
        initial_fields(),
        settable_field(),
        tag(),
        any::<String>(),
        identity(),
        any::<([u8; 32], [u8; 32])>().prop_filter("distinct edge endpoints", |(a, b)| a != b),
        prop::sample::select(vec![EdgeKind::Parent, EdgeKind::Blocks, EdgeKind::Relates]),
    )
        .prop_map(
            |(title, fields, (field, value), tag, text, who, (task, to), kind)| {
                let task = TaskId::from_bytes(task);
                let to = TaskId::from_bytes(to);
                [
                    Event::TaskCreated { title, fields },
                    Event::TaskFieldSet { task, field, value },
                    Event::TagAdded {
                        task,
                        tag: tag.clone(),
                    },
                    Event::TagRemoved { task, tag },
                    Event::AssigneeAdded { task, who },
                    Event::AssigneeRemoved { task, who },
                    Event::EdgeAdded {
                        from: task,
                        kind,
                        to,
                    },
                    Event::EdgeRemoved {
                        from: task,
                        kind,
                        to,
                    },
                    Event::CommentAdded { task, text },
                    Event::TaskDeleted { task },
                ]
            },
        )
}

proptest! {
    #[test]
    fn every_event_variant_round_trips(
        events in events(),
        author in any::<[u8; 32]>(),
        external_id in any::<[u8; 32]>(),
        (wall_ms, counter) in any::<(u64, u32)>(),
    ) {
        let author = DeviceId::from_bytes(author);
        let id = EventId::from_bytes(external_id);
        let hlc = Hlc { wall_ms, counter };
        for event in events {
            let local = Envelope::new(author, hlc, event.clone()).unwrap();
            let bytes = local.encode().unwrap();
            let decoded = Envelope::decode(&bytes).unwrap();
            prop_assert_eq!(&decoded, &DecodeOutcome::Known(local));
            let DecodeOutcome::Known(decoded) = decoded else { unreachable!() };
            let reencoded = decoded.encode().unwrap();
            prop_assert_eq!(&reencoded, &bytes);
            prop_assert_eq!(
                Envelope::decode_with_id(id, &bytes).unwrap(),
                DecodeOutcome::Known(Envelope::with_id(id, author, hlc, event)),
            );
        }
    }
}

#[test]
fn rejects_declared_lengths_without_data_or_excessive_nesting() {
    for major in [0x5b, 0x7b, 0x9b, 0xbb] {
        let bytes = [&[0x82, 1, major][..], &[0xff; 8]].concat();
        assert!(matches!(Envelope::decode(&bytes), Err(DecodeError::Cbor)));
    }
    let deep = [&[0x82, 1][..], &[0x81; 512], &[0xf6]].concat();
    assert!(matches!(Envelope::decode(&deep), Err(DecodeError::Cbor)));
}

proptest! {
    #[test]
    fn arbitrary_bytes_decode_without_panicking(bytes in prop::collection::vec(any::<u8>(), 0..1024)) {
        for candidate in [bytes.clone(), [&[0x82, 1], bytes.as_slice()].concat()] {
            if let Ok(DecodeOutcome::Known(envelope)) = Envelope::decode(&candidate) {
                prop_assert_eq!(envelope.encode().unwrap(), candidate);
            }
        }
    }

    #[test]
    fn single_bit_flips_cannot_preserve_the_id_of_a_known_event(
        events in events(),
        author in any::<[u8; 32]>(),
        (wall_ms, counter) in any::<(u64, u32)>(),
        index in any::<prop::sample::Index>(),
        bit in 0u8..8,
    ) {
        let author = DeviceId::from_bytes(author);
        for event in events {
            let envelope = Envelope::new(author, Hlc { wall_ms, counter }, event).unwrap();
            let mut bytes = envelope.encode().unwrap();
            let index = index.index(bytes.len());
            bytes[index] ^= 1 << bit;
            if let Ok(DecodeOutcome::Known(changed)) = Envelope::decode(&bytes) {
                prop_assert_ne!(changed.id, envelope.id);
                prop_assert_eq!(changed.encode().unwrap(), bytes);
            }
        }
    }
}

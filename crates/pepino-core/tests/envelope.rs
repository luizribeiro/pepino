use pepino_core::*;
use proptest::prelude::*;

#[test]
fn round_trips_task_creation_with_local_or_external_id() {
    let author = DeviceId::from_bytes([31; 32]);
    let hlc = Hlc {
        wall_ms: 1_795_144_800_000,
        counter: 2,
    };
    let event = Event::TaskCreated {
        title: "Repair the garden gate".into(),
        fields: InitialFields {
            due: Date::from_ymd_opt(2026, 12, 1),
            tags: ["garden".into(), "repairs".into()].into(),
            ..Default::default()
        },
    };
    let local = Envelope::new(author, hlc, event.clone()).unwrap();
    let external = Envelope::with_id(EventId::from_bytes([19; 32]), author, hlc, event);
    let encoded = local.encode().unwrap();
    assert_eq!(
        Envelope::decode(&encoded).unwrap(),
        DecodeOutcome::Known(local)
    );
    assert_eq!(
        Envelope::decode_with_id(external.id, &encoded).unwrap(),
        DecodeOutcome::Known(external)
    );
}

#[test]
fn unknown_version_still_requires_a_complete_frame() {
    let mut bytes = Vec::new();
    ciborium::into_writer(&(2, "future content format"), &mut bytes).unwrap();
    assert_eq!(
        Envelope::decode(&bytes).unwrap(),
        DecodeOutcome::UnknownVersion { version: 2 }
    );
    assert!(matches!(
        Envelope::decode(&bytes[..bytes.len() - 1]),
        Err(DecodeError::Cbor)
    ));
    bytes.push(0);
    assert!(matches!(
        Envelope::decode(&bytes),
        Err(DecodeError::TrailingData)
    ));
}

#[test]
fn rejects_invalid_cbor_and_invalid_known_content() {
    assert!(matches!(Envelope::decode(&[0xff]), Err(DecodeError::Cbor)));
    assert!(matches!(
        Envelope::decode(&[0x81, 1]),
        Err(DecodeError::InvalidFrame)
    ));
    assert!(matches!(
        Envelope::decode(&[0x82, 1, 0xf6]),
        Err(DecodeError::InvalidContent)
    ));
}

proptest! {
    #[test]
    fn task_field_set_round_trips(
        author in any::<[u8; 32]>(), task in any::<[u8; 32]>(),
        wall_ms in any::<u64>(), counter in any::<u32>(),
        value in prop_oneof![
            any::<String>().prop_map(FieldValue::Text),
            any::<i64>().prop_map(FieldValue::Integer),
            Just(FieldValue::Date(Date::from_ymd_opt(2026, 12, 1).unwrap())),
            Just(FieldValue::Status(Status::Doing)),
            Just(FieldValue::Unset),
        ]
    ) {
        let event = Event::TaskFieldSet {
            task: TaskId(EventId::from_bytes(task)),
            field: FieldName::new("review"),
            value,
        };
        let envelope = Envelope::new(DeviceId::from_bytes(author), Hlc { wall_ms, counter }, event).unwrap();
        let bytes = envelope.encode().unwrap();
        prop_assert_eq!(Envelope::decode(&bytes).unwrap(), DecodeOutcome::Known(envelope));
    }
}

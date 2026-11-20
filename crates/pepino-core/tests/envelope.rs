use pepino_core::*;
use proptest::prelude::*;

#[test]
fn round_trips_task_creation_with_a_content_id() {
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
    let envelope = Envelope::new(author, hlc, event.clone()).unwrap();
    assert_eq!(envelope.created_task(), Some(TaskId(envelope.id())));
    assert_eq!(envelope.author(), author);
    assert_eq!(envelope.hlc(), hlc);
    assert_eq!(envelope.event(), &event);
    let encoded = envelope.encode().unwrap();
    let decoded = Envelope::decode(&encoded).unwrap();
    assert_eq!(decoded.id().as_bytes(), blake3::hash(&encoded).as_bytes());
    assert_eq!(decoded.author(), Some(author));
    assert_eq!(decoded.hlc(), Some(hlc));
    assert_eq!(decoded, DecodeOutcome::Known(envelope));
}

#[test]
fn unknown_version_still_requires_a_complete_frame() {
    let mut bytes = Vec::new();
    ciborium::into_writer(&(2, "future content format", true), &mut bytes).unwrap();
    let decoded = Envelope::decode(&bytes).unwrap();
    let id = EventId::from_bytes(*blake3::hash(&bytes).as_bytes());
    assert_eq!(decoded.id(), id);
    assert_eq!(decoded.author(), None);
    assert_eq!(decoded.hlc(), None);
    assert_eq!(
        decoded,
        DecodeOutcome::UnknownVersion {
            version: 2,
            id,
            author: None,
            hlc: None,
            bytes: bytes.clone(),
        }
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
fn future_frames_preserve_metadata_when_available() {
    let author = DeviceId::from_bytes([12; 32]);
    let hlc = Hlc {
        wall_ms: 45,
        counter: 2,
    };
    let content = ciborium::Value::Map(vec![
        (
            "author".into(),
            ciborium::Value::serialized(&author).unwrap(),
        ),
        ("hlc".into(), ciborium::Value::serialized(&hlc).unwrap()),
    ]);
    let mut bytes = Vec::new();
    ciborium::into_writer(&(12, content, "future payload"), &mut bytes).unwrap();
    let id = EventId::from_bytes(*blake3::hash(&bytes).as_bytes());
    let decoded = Envelope::decode(&bytes).unwrap();
    assert_eq!(decoded.id(), id);
    assert_eq!(decoded.author(), Some(author));
    assert_eq!(decoded.hlc(), Some(hlc));
    assert_eq!(
        decoded,
        DecodeOutcome::UnknownVersion {
            version: 12,
            id,
            author: Some(author),
            hlc: Some(hlc),
            bytes,
        }
    );
    assert!(matches!(
        Envelope::decode(&[0x81, 2]),
        Ok(DecodeOutcome::UnknownVersion { version: 2, .. })
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
    fn envelope_order_key_is_a_total_order(raw in any::<[(u64, u32, [u8; 32]); 3]>()) {
        use std::cmp::Ordering;

        let task = TaskId::from_bytes([19; 32]);
        for shared_prefix in 0..=3 {
            let raw_keys = raw.map(|(mut wall_ms, mut counter, mut id)| {
                if shared_prefix >= 1 { wall_ms = raw[0].0; }
                if shared_prefix >= 2 { counter = raw[0].1; }
                if shared_prefix >= 3 { id = raw[0].2; }
                (wall_ms, counter, id)
            });
            let envelopes = raw_keys.map(|(wall_ms, counter, author)| {
                Envelope::new(
                    DeviceId::from_bytes(author), Hlc { wall_ms, counter }, Event::TaskDeleted { task },
                ).unwrap()
            });
            let keys = envelopes.each_ref().map(Envelope::order_key);

            for (i, j, k) in [(0, 1, 2), (1, 2, 0), (2, 0, 1)] {
                let (a, b, c) = (keys[i], keys[j], keys[k]);
                let order = a.cmp(&b);
                prop_assert_eq!(order, (envelopes[i].hlc(), envelopes[i].id()).cmp(&(envelopes[j].hlc(), envelopes[j].id())));
                prop_assert_eq!(order, b.cmp(&a).reverse());
                prop_assert_eq!(order == Ordering::Equal, a == b);
                if a <= b && b <= c {
                    prop_assert!(a <= c);
                }
                if a >= b && b >= c {
                    prop_assert!(a >= c);
                }
            }
        }
    }
}

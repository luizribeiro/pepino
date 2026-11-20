use ciborium::Value;
use pepino_core::*;

#[test]
fn rejects_equivalent_cbor_encodings() {
    let envelope = Envelope::new(
        DeviceId::from_bytes([31; 32]),
        Hlc {
            wall_ms: 27,
            counter: 2,
        },
        Event::TagAdded {
            task: TaskId(EventId::from_bytes([15; 32])),
            tag: "garden".into(),
        },
    )
    .unwrap();
    let bytes = envelope.encode().unwrap();

    let non_minimal = [&[0x82, 0x18, 1], &bytes[2..]].concat();
    let indefinite = [&[0x9f], &bytes[1..], &[0xff]].concat();
    for alternate in [non_minimal, indefinite] {
        assert!(matches!(
            Envelope::decode(&alternate),
            Err(DecodeError::NonCanonical)
        ));
        assert!(matches!(
            Envelope::decode_with_id(envelope.id(), &alternate),
            Err(DecodeError::NonCanonical)
        ));
    }

    let frame: Value = ciborium::from_reader(bytes.as_slice()).unwrap();
    for change in [
        |fields: &mut Vec<(Value, Value)>| fields.swap(0, 1),
        |fields: &mut Vec<(Value, Value)>| fields.push(fields[0].clone()),
        |fields: &mut Vec<(Value, Value)>| fields.push(("extra".into(), true.into())),
    ] {
        let mut frame = frame.clone();
        change(frame.as_array_mut().unwrap()[1].as_map_mut().unwrap());
        let mut alternate = Vec::new();
        ciborium::into_writer(&frame, &mut alternate).unwrap();
        assert!(matches!(
            Envelope::decode(&alternate),
            Err(DecodeError::NonCanonical)
        ));
    }
}

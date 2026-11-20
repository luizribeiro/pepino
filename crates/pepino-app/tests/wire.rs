use std::fmt::Debug;

use pepino_app::{
    AppError, EdgeExclusionReason, PROTOCOL_VERSION, Request, Response, WireFormat, WireRequest,
    WireResponse,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

fn round_trip<T: Debug + Eq + DeserializeOwned + Serialize>(value: &T) {
    let json = serde_json::to_vec(value).unwrap();
    assert_eq!(serde_json::from_slice::<T>(&json).unwrap(), *value);
    let mut cbor = Vec::new();
    ciborium::into_writer(value, &mut cbor).unwrap();
    assert_eq!(
        ciborium::from_reader::<T, _>(cbor.as_slice()).unwrap(),
        *value
    );
}

#[test]
fn every_request_has_a_stable_operation_tag() {
    let fixtures = [
        json!({"op":"add_task","title":"Replace the greenhouse latch","due":"2026-12-01",
            "tags":["greenhouse"],"fields":{"estimate":{"type":"integer","value":3}},"assignees":["me"]}),
        json!({"op":"set_fields","task":"97ad","changes":{"due":{"type":"unset"},"status":{"type":"status","value":"done"}}}),
        json!({"op":"add_tags","task":"97ad","tags":["garden"]}),
        json!({"op":"remove_tags","task":"97ad","tags":["garden"]}),
        json!({"op":"assign","task":"97ad","assignees":["me"]}),
        json!({"op":"unassign","task":"97ad","assignees":["me"]}),
        json!({"op":"comment","task":"97ad","text":"The new latch arrived."}),
        json!({"op":"link","from":"97ad","kind":"blocks","to":"33ea"}),
        json!({"op":"unlink","from":"97ad","kind":"relates","to":"33ea"}),
        json!({"op":"delete","task":"97ad"}),
        json!({"op":"get_task","task":"97ad"}),
    ];
    for fixture in fixtures {
        let request = serde_json::from_value::<Request>(fixture.clone()).unwrap();
        let encoded = serde_json::to_value(&request).unwrap();
        for (key, expected) in fixture.as_object().unwrap() {
            assert_eq!(&encoded[key], expected, "{key}");
        }
        round_trip(&request);
    }
}

#[test]
fn errors_and_write_responses_have_stable_tags() {
    for fixture in [
        json!({"kind":"not_found","task":"97ad"}),
        json!({"kind":"ambiguous_id","task":"97ad","count":2,"candidates":["97ada","97adb"]}),
        json!({"kind":"invalid_input","field":"tag","reason":"invalid tag"}),
        json!({"kind":"conflict","task":"97ada"}),
        json!({"kind":"busy"}),
        json!({"kind":"storage","retryable":true,"reason":"database is busy"}),
        json!({"kind":"unsupported_version","requested":2,"supported":1}),
    ] {
        let error = serde_json::from_value::<AppError>(fixture.clone()).unwrap();
        assert_eq!(serde_json::to_value(error.kind()).unwrap(), fixture["kind"]);
        assert_eq!(error.kind().code(), fixture["kind"].as_str().unwrap());
        assert_eq!(serde_json::to_value(&error).unwrap(), fixture);
        round_trip(&error);
    }
    let response = Response::Write {
        changed: true,
        created: Some("97ad".repeat(16)),
        tasks: Vec::new(),
        edges: Vec::new(),
    };
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(
        json,
        json!({"kind":"write","changed":true,"created":"97ad".repeat(16),"tasks":[],"edges":[]})
    );
    round_trip(&response);
}

#[test]
fn exclusion_reasons_have_stable_names() {
    for (reason, expected) in [
        (EdgeExclusionReason::MissingEndpoint, "missing_endpoint"),
        (EdgeExclusionReason::SupersededParent, "superseded_parent"),
        (EdgeExclusionReason::Cycle, "cycle"),
    ] {
        assert_eq!(serde_json::to_value(reason).unwrap(), expected);
        round_trip(&reason);
    }
}

#[test]
fn versioned_messages_have_a_shared_json_and_cbor_codec() {
    let request = WireRequest::new(Request::GetTask {
        task: "97ad".into(),
    });
    let ok = WireResponse::new(Ok(Response::Write {
        changed: false,
        created: None,
        tasks: Vec::new(),
        edges: Vec::new(),
    }));
    let error = WireResponse::new(Err(AppError::NotFound {
        task: "97ad".into(),
    }));

    for format in [WireFormat::Json, WireFormat::Cbor] {
        assert_eq!(
            format
                .decode::<WireRequest>(&format.encode(&request).unwrap())
                .unwrap(),
            request
        );
        for response in [&ok, &error] {
            assert_eq!(
                &format
                    .decode::<WireResponse>(&format.encode(response).unwrap())
                    .unwrap(),
                response
            );
        }
    }
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({"v":1,"request":{"op":"get_task","task":"97ad"}})
    );
    assert_eq!(
        serde_json::to_value(ok).unwrap(),
        json!({"v":1,"result":{"ok":{"kind":"write","changed":false,"tasks":[],"edges":[]}}})
    );
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        json!({"v":1,"result":{"error":{"kind":"not_found","task":"97ad"}}})
    );
}

#[test]
fn both_codecs_check_the_version_before_the_operation_and_reject_extra_fields() {
    for format in [WireFormat::Json, WireFormat::Cbor] {
        let future = format
            .encode(&json!({"v":PROTOCOL_VERSION+1,"request":{"op":"reorder_board"}}))
            .unwrap();
        assert_eq!(
            format.decode::<WireRequest>(&future),
            Err(AppError::UnsupportedVersion {
                requested: PROTOCOL_VERSION + 1,
                supported: PROTOCOL_VERSION,
            })
        );
        for invalid in [
            json!({"v":1,"request":{"op":"add_task","title":"Replace the latch","tag":["greenhouse"]}}),
            json!({"v":1,"request":{"op":"get_task","task":"97ad","tags":[]} }),
            json!({"v":1,"request":{"op":"get_task","task":"97ad"},"extra":true}),
        ] {
            let bytes = format.encode(&invalid).unwrap();
            assert!(matches!(
                format.decode::<WireRequest>(&bytes),
                Err(AppError::InvalidInput { .. })
            ));
        }
        let mut trailing = format
            .encode(&WireRequest::new(Request::GetTask {
                task: "97ad".into(),
            }))
            .unwrap();
        trailing.push(0);
        assert!(matches!(
            format.decode::<WireRequest>(&trailing),
            Err(AppError::InvalidInput { .. })
        ));
    }
}

#[test]
fn status_and_edge_kinds_are_protocol_strings() {
    use pepino_app::{EdgeKind, Status};

    for (status, label) in [
        (Status::Todo, "todo"),
        (Status::Doing, "doing"),
        (Status::Done, "done"),
        (Status::Cancelled, "cancelled"),
    ] {
        assert_eq!(serde_json::to_value(status).unwrap(), label);
        assert_eq!(
            Status::try_from(pepino_core::Status::from(status)).unwrap(),
            status
        );
        round_trip(&status);
    }
    for (kind, label) in [
        (EdgeKind::Parent, "parent"),
        (EdgeKind::Blocks, "blocks"),
        (EdgeKind::Relates, "relates"),
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), label);
        assert_eq!(
            EdgeKind::try_from(pepino_core::EdgeKind::from(kind)).unwrap(),
            kind
        );
        round_trip(&kind);
    }
}

#[test]
fn dates_and_field_values_have_fixed_wire_shapes() {
    use pepino_app::{Date, FieldValue, Status};

    let date: Date = "2026-12-01".parse().unwrap();
    assert_eq!(serde_json::to_value(date).unwrap(), "2026-12-01");
    round_trip(&date);
    for (value, expected) in [
        (
            FieldValue::Text("Use stainless hardware.".into()),
            json!({"type":"text","value":"Use stainless hardware."}),
        ),
        (FieldValue::Integer(3), json!({"type":"integer","value":3})),
        (
            FieldValue::Date(date),
            json!({"type":"date","value":"2026-12-01"}),
        ),
        (
            FieldValue::Status(Status::Done),
            json!({"type":"status","value":"done"}),
        ),
        (FieldValue::Unset, json!({"type":"unset"})),
    ] {
        assert_eq!(serde_json::to_value(&value).unwrap(), expected);
        assert_eq!(
            FieldValue::try_from(pepino_core::FieldValue::from(value.clone())).unwrap(),
            value
        );
        round_trip(&value);
    }
    assert!("2026-02-29".parse::<Date>().is_err());
    assert!(
        serde_json::from_value::<FieldValue>(json!({"type":"integer","value":3,"unit":"hours"}))
            .is_err()
    );
}

#[test]
fn clocks_preserve_raw_values_alongside_rfc3339_utc() {
    use pepino_app::Hlc;

    for (wall_ms, utc) in [
        (1_795_144_800_123, json!("2026-11-20T03:20:00.123Z")),
        (253_402_300_799_999, json!("9999-12-31T23:59:59.999Z")),
        (253_402_300_800_000, json!(null)),
        (u64::MAX, json!(null)),
    ] {
        let core = pepino_core::Hlc {
            wall_ms,
            counter: 7,
        };
        let clock = Hlc::from(core);
        assert_eq!(
            serde_json::to_value(clock).unwrap(),
            json!({"wall_ms":wall_ms,"counter":7,"utc":utc})
        );
        assert_eq!(pepino_core::Hlc::from(clock), core);
        round_trip(&clock);
    }
}

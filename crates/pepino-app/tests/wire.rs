use std::fmt::Debug;

use pepino_app::{AppError, Request, Response};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

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
            "tags":["greenhouse"],"fields":{"estimate":{"integer":3}},"assignees":["me"]}),
        json!({"op":"set_fields","task":"97ad","changes":{"due":"unset","status":{"status":"done"}}}),
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
        json!({"kind":"storage","retryable":true,"reason":"database is busy"}),
    ] {
        let error = serde_json::from_value::<AppError>(fixture.clone()).unwrap();
        assert_eq!(serde_json::to_value(error.kind()).unwrap(), fixture["kind"]);
        assert_eq!(error.kind().code(), fixture["kind"].as_str().unwrap());
        assert_eq!(serde_json::to_value(&error).unwrap(), fixture);
        round_trip(&error);
    }
    let response = Response::Write {
        changed: true,
        task_ids: vec!["97ad".repeat(16)],
        tasks: Vec::new(),
    };
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["kind"], Value::from("write"));
    assert_eq!(json["changed"], Value::from(true));
    round_trip(&response);
}

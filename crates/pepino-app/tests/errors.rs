use std::fs::File;

use pepino_app::{App, AppError, AppErrorKind, Hlc};
use pepino_core::{DeviceId, Envelope, Event, EventId, InitialFields};
use pepino_store::{IdSource, Store};
use serde_json::{Value, json};

const NOW: u64 = 1_795_144_800_000;

fn operations(reference: &str, peer: &str) -> [Value; 11] {
    [
        json!({"op":"add_task","title":"Install the replacement latch","parent":reference}),
        json!({"op":"set_fields","task":reference,"changes":{"status":{"status":"done"}}}),
        json!({"op":"add_tags","task":reference,"tags":["greenhouse"]}),
        json!({"op":"remove_tags","task":reference,"tags":["greenhouse"]}),
        json!({"op":"assign","task":reference,"assignees":["me"]}),
        json!({"op":"unassign","task":reference,"assignees":["me"]}),
        json!({"op":"comment","task":reference,"text":"The latch arrived."}),
        json!({"op":"link","from":reference,"kind":"blocks","to":peer}),
        json!({"op":"unlink","from":peer,"kind":"blocks","to":reference}),
        json!({"op":"delete","task":reference}),
        json!({"op":"get_task","task":reference}),
    ]
}

#[tokio::test]
async fn every_operation_classifies_task_references_and_storage_contention() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    let board = app.local_state().board_id;
    let store = Store::open(directory.path().join("pepino.sqlite"))
        .await
        .unwrap();

    let mut ids = Vec::new();
    for (bytes, title) in [
        ([0xab, 0xcd, 0xf1], "Order the latch"),
        ([0xab, 0xcd, 0xf2], "Order the hinges"),
        ([0xbe, 0xef, 0x81], "Weatherproof the door"),
    ] {
        let mut id = [31; 32];
        id[..3].copy_from_slice(&bytes);
        let id = EventId::from_bytes(id);
        let envelope = Envelope::new(
            DeviceId::from_bytes([41; 32]),
            Hlc {
                wall_ms: NOW - 10,
                counter: 0,
            },
            Event::TaskCreated {
                title: title.into(),
                fields: InitialFields::default(),
            },
        )
        .unwrap();
        store
            .append(board, &envelope.encode().unwrap(), IdSource::Trusted(id))
            .await
            .unwrap();
        ids.push(id.to_string());
    }
    let (left, right, peer) = (&ids[0], &ids[1], &ids[2]);
    let before = store.latest_hlc(board).await.unwrap();

    for (reference, kind) in [
        ("g123", AppErrorKind::InvalidInput),
        ("0000", AppErrorKind::NotFound),
        ("ABCd", AppErrorKind::AmbiguousId),
    ] {
        for request in operations(reference, peer) {
            let error = app
                .handle(serde_json::from_value(request).unwrap(), NOW)
                .await
                .unwrap_err();
            assert_eq!(error.kind(), kind);
            if kind == AppErrorKind::AmbiguousId {
                assert_eq!(
                    error,
                    AppError::AmbiguousId {
                        task: reference.into(),
                        count: 2,
                        candidates: vec![left.clone(), right.clone()],
                    }
                );
            }
        }
    }
    assert_eq!(store.latest_hlc(board).await.unwrap(), before);

    let path = directory.path().join("local.json");
    let guard = File::options().read(true).write(true).open(path).unwrap();
    guard.try_lock().unwrap();
    for request in operations(left, peer) {
        let error = app
            .handle(serde_json::from_value(request).unwrap(), NOW)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            AppError::Storage {
                retryable: true,
                ..
            }
        ));
    }
    drop(guard);
    assert_eq!(store.latest_hlc(board).await.unwrap(), before);

    for id in [left, right] {
        let delete = json!({"op":"delete","task":id});
        app.handle(serde_json::from_value(delete).unwrap(), NOW)
            .await
            .unwrap();
    }
    let before = store.latest_hlc(board).await.unwrap();
    for request in operations("abcdf1", peer) {
        let error = app
            .handle(serde_json::from_value(request).unwrap(), NOW)
            .await
            .unwrap_err();
        assert_eq!(error, AppError::Conflict { task: left.clone() });
    }
    let request = json!({"op":"get_task","task":"abcd"});
    assert_eq!(
        app.handle(serde_json::from_value(request).unwrap(), NOW)
            .await
            .unwrap_err(),
        AppError::AmbiguousId {
            task: "abcd".into(),
            count: 2,
            candidates: vec![left.clone(), right.clone()]
        },
    );
    assert_eq!(store.latest_hlc(board).await.unwrap(), before);
}

#[tokio::test]
async fn creation_rejects_invalid_events_without_writing_history() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), NOW).await.unwrap();
    for (input, expected_field) in [
        (json!({"title":""}), "title"),
        (json!({"title":"x".repeat(1025)}), "title"),
        (json!({"description":"x".repeat(65_537)}), "description"),
        (json!({"tags":["bad TAG"]}), "tags"),
        (json!({"assignees":["Ada"]}), "assignees"),
        (
            json!({"fields":{"status":{"status":"done"}}}),
            "fields.status",
        ),
        (
            json!({"fields":{"malformed/name":{"integer":3}}}),
            "fields.malformed/name",
        ),
    ] {
        let mut request = json!({"op":"add_task","title":"Install the replacement latch"});
        request
            .as_object_mut()
            .unwrap()
            .extend(input.as_object().unwrap().clone());
        let error = app
            .handle(serde_json::from_value(request).unwrap(), NOW)
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::InvalidInput { field, .. } if field == expected_field));
    }
    let store = Store::open(directory.path().join("pepino.sqlite"))
        .await
        .unwrap();
    assert_eq!(
        store.latest_hlc(app.local_state().board_id).await.unwrap(),
        None
    );
}

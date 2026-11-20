use std::fs::File;
use std::time::Duration;

use pepino_app::{App, AppError, AppErrorKind, OpenOptions};
use pepino_core::{DeviceId, Envelope, Event, Hlc, InitialFields};
use pepino_store::{IdSource, Store};
use serde_json::{Value, json};

const NOW: u64 = 1_795_144_800_000;

fn operations(reference: &str, peer: &str) -> [(Value, &'static str); 15] {
    [
        (
            json!({"op":"add_task","title":"Install the replacement latch","parent":reference}),
            "parent",
        ),
        (
            json!({"op":"add_task","title":"Install the replacement latch","blocks":[peer, reference]}),
            "blocks[1]",
        ),
        (
            json!({"op":"set_fields","task":reference,"changes":{"status":{"type":"status","value":"done"}}}),
            "task",
        ),
        (
            json!({"op":"add_tags","task":reference,"tags":["greenhouse"]}),
            "task",
        ),
        (
            json!({"op":"remove_tags","task":reference,"tags":["greenhouse"]}),
            "task",
        ),
        (
            json!({"op":"assign","task":reference,"assignees":["me"]}),
            "task",
        ),
        (
            json!({"op":"unassign","task":reference,"assignees":["me"]}),
            "task",
        ),
        (
            json!({"op":"comment","task":reference,"text":"The latch arrived."}),
            "task",
        ),
        (
            json!({"op":"link","from":reference,"kind":"blocks","to":peer}),
            "from",
        ),
        (
            json!({"op":"unlink","from":peer,"kind":"blocks","to":reference}),
            "to",
        ),
        (json!({"op":"delete","task":reference}), "task"),
        (json!({"op":"get_task","task":reference}), "task"),
        (json!({"op":"tree","task":reference}), "task"),
        (json!({"op":"deps","task":reference}), "task"),
        (json!({"op":"log","task":reference}), "task"),
    ]
}

#[tokio::test]
async fn every_operation_classifies_task_references_and_storage_contention() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::init_with_options(
        directory.path(),
        NOW,
        OpenOptions {
            lock_timeout: Duration::ZERO,
            ..OpenOptions::default()
        },
    )
    .await
    .unwrap();
    let board = app.local_state().board_id;
    let store = Store::open(directory.path().join("pepino.sqlite"))
        .await
        .unwrap();

    let mut ids = Vec::new();
    for (nonce, title) in [
        (24312_u32, "Order the latch"),
        (37367, "Order the hinges"),
        (0, "Weatherproof the door"),
    ] {
        let mut author = [41; 32];
        author[..4].copy_from_slice(&nonce.to_le_bytes());
        let envelope = Envelope::new(
            DeviceId::from_bytes(author),
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
            .append(board, &envelope.encode().unwrap(), IdSource::Content)
            .await
            .unwrap();
        ids.push(envelope.id().to_string());
    }
    let (left, right, peer) = (&ids[0], &ids[1], &ids[2]);
    let before = store.latest_hlc(board).await.unwrap();

    let request = json!({"op":"list_tasks","filter":"parent:abcd","today":"2026-11-20"});
    let error = app
        .handle(serde_json::from_value(request).unwrap(), NOW)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, AppError::AmbiguousId { field, task, count: 2, candidates, diagnostic: Some(diagnostic) }
        if field == "filter" && task == "abcd" && candidates == &vec![right.clone(), left.clone()]
            && diagnostic.term == "parent:abcd")
    );

    for (reference, kind) in [
        ("g123", AppErrorKind::InvalidInput),
        ("0000", AppErrorKind::NotFound),
        ("ABCd", AppErrorKind::AmbiguousId),
    ] {
        for (request, field) in operations(reference, peer) {
            let error = app
                .handle(serde_json::from_value(request).unwrap(), NOW)
                .await
                .unwrap_err();
            assert_eq!(error.kind(), kind);
            if kind == AppErrorKind::AmbiguousId {
                assert_eq!(
                    error,
                    AppError::AmbiguousId {
                        field: field.into(),
                        task: reference.into(),
                        count: 2,
                        candidates: vec![right.clone(), left.clone()],
                        diagnostic: None,
                    }
                );
            } else if kind == AppErrorKind::NotFound {
                assert_eq!(
                    error,
                    AppError::NotFound {
                        field: field.into(),
                        reference: pepino_app::MissingReference::Task(reference.into())
                    }
                );
            } else {
                assert!(
                    matches!(error, AppError::InvalidInput { field: location, .. } if location == field)
                );
            }
        }
    }
    assert_eq!(store.latest_hlc(board).await.unwrap(), before);

    let path = directory.path().join("local.lock");
    let guard = File::options().read(true).write(true).open(path).unwrap();
    guard.try_lock().unwrap();
    for (request, _) in operations(left, peer) {
        let error = app
            .handle(serde_json::from_value(request).unwrap(), NOW)
            .await
            .unwrap_err();
        assert_eq!(error, AppError::Busy);
        assert!(error.retryable());
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
    for (request, field) in operations(&left[..6], peer) {
        if request["op"] == "delete" {
            let outcome = app
                .handle(serde_json::from_value(request).unwrap(), NOW)
                .await
                .unwrap();
            assert!(matches!(
                outcome,
                pepino_app::Response::Write { changed: false, .. }
            ));
            continue;
        }
        let conflict = matches!(request["op"].as_str(), Some("add_task" | "link"));
        let error = app
            .handle(serde_json::from_value(request).unwrap(), NOW)
            .await
            .unwrap_err();
        let task = left.clone();
        let title = Some("Order the latch".into());
        assert_eq!(
            error,
            if conflict {
                AppError::Conflict {
                    field: field.into(),
                    reference: pepino_app::ConflictReference::Task { task, title },
                }
            } else {
                AppError::NotFound {
                    field: field.into(),
                    reference: pepino_app::MissingReference::DeletedTask { task, title },
                }
            }
        );
    }
    for request in [
        json!({"op":"get_task","task":"abcd"}),
        json!({"op":"delete","task":"abcd"}),
    ] {
        assert_eq!(
            app.handle(serde_json::from_value(request).unwrap(), NOW)
                .await
                .unwrap_err(),
            AppError::NotFound {
                field: "task".into(),
                reference: pepino_app::MissingReference::DeletedTask {
                    task: "abcd".into(),
                    title: None,
                },
            },
        );
    }
    assert_eq!(store.latest_hlc(board).await.unwrap(), before);
}

#[tokio::test]
async fn creation_rejects_invalid_events_without_writing_history() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::init(directory.path(), NOW).await.unwrap();
    for (input, expected_field) in [
        (json!({"title":""}), "title"),
        (json!({"title":"x".repeat(1025)}), "title"),
        (json!({"description":"x".repeat(65_537)}), "description"),
        (json!({"tags":["greenhouse", "bad TAG"]}), "tags[1]"),
        (json!({"assignees":["me", "Ada"]}), "assignees[1]"),
        (
            json!({"fields":{"status":{"type":"status","value":"done"}}}),
            "fields.status",
        ),
        (
            json!({"fields":{"malformed/name":{"type":"integer","value":3}}}),
            r#"fields["malformed/name"]"#,
        ),
        (
            json!({"fields":{"estimate":{"type":"text","value":"x".repeat(65_537)}}}),
            "fields.estimate",
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

#[tokio::test]
async fn opening_requires_both_local_files_and_a_matching_binding() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("greenhouse");
    let error = App::open(&path, NOW).await.err().unwrap();
    assert_eq!(error.kind(), AppErrorKind::NotInitialized);
    assert!(!path.exists());

    let app = App::init(&path, NOW).await.unwrap();
    let local = path.join("local.json");
    let original = std::fs::read(&local).unwrap();
    let mut state: Value = serde_json::from_slice(&original).unwrap();
    drop(app);
    state["board_id"] = json!("f1".repeat(32));
    std::fs::write(&local, serde_json::to_vec(&state).unwrap()).unwrap();
    let error = App::open(&path, NOW).await.err().unwrap();
    assert!(error.to_string().contains("database does not match"));

    std::fs::remove_file(&local).unwrap();
    let error = App::init(&path, NOW).await.err().unwrap();
    assert!(error.to_string().contains("pepino.sqlite exists without"));
    assert!(!local.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn app_reports_the_path_of_unreadable_local_state() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let directory = tempfile::tempdir().unwrap();
    App::init(directory.path(), NOW).await.unwrap();
    let path = directory.path().join("local.json");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let result = App::open(directory.path(), NOW).await;
    let os_can_read = fs::read(&path).is_ok();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    if !os_can_read {
        assert!(
            matches!(result, Err(AppError::PermissionDenied { path: denied, .. })
            if denied == path.display().to_string())
        );
    }
}

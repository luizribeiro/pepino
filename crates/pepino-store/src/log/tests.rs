use ciborium::Value;
use pepino_core::{Event, InitialFields};

use super::*;

fn creation() -> Envelope {
    Envelope::new(
        DeviceId::from_bytes([51; 32]),
        Hlc {
            wall_ms: 1_795_144_800_000,
            counter: 3,
        },
        Event::TaskCreated {
            title: "Replace the greenhouse latch".into(),
            fields: InitialFields::default(),
        },
    )
    .unwrap()
}

fn property<'a>(map: &'a mut Value, name: &str) -> &'a mut Value {
    &mut map
        .as_map_mut()
        .unwrap()
        .iter_mut()
        .find(|(key, _)| key.as_text() == Some(name))
        .unwrap()
        .1
}

#[tokio::test]
async fn retains_each_skipped_classification_and_exact_bytes() {
    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([19; 32]);
    let original = creation();
    let bytes = original.encode().unwrap();
    for (index, (class, expected)) in [
        ("unknown_version", SkipReason::UnknownVersion(2)),
        (
            "unknown_event",
            SkipReason::UnknownEvent("reminder_added".into()),
        ),
        (
            "unknown_value",
            SkipReason::UnknownValue {
                variant: "task_created".into(),
                values: vec![UnknownValue::Status("waiting".into())],
            },
        ),
        ("invalid", SkipReason::Invalid(InvalidEvent::EmptyTitle)),
    ]
    .into_iter()
    .enumerate()
    {
        let mut value: Value = ciborium::from_reader(bytes.as_slice()).unwrap();
        let frame = value.as_array_mut().unwrap();
        if index == 0 {
            frame[0] = Value::from(2);
        } else {
            let event = property(&mut frame[1], "event");
            let (tag, payload) = &mut event.as_map_mut().unwrap()[0];
            match index {
                1 => *tag = Value::from("reminder_added"),
                2 => *property(property(payload, "fields"), "status") = Value::from("waiting"),
                3 => *property(payload, "title") = Value::from(""),
                _ => unreachable!(),
            }
        }
        let mut raw = Vec::new();
        ciborium::into_writer(&value, &mut raw).unwrap();
        let id = Envelope::decode(&raw).unwrap().id();
        assert_eq!(
            store.append(board, &raw, IdSource::Content).await.unwrap(),
            AppendOutcome::Skipped {
                id,
                reason: expected
            }
        );
        assert_eq!(
            store.append(board, &raw, IdSource::Content).await.unwrap(),
            AppendOutcome::Duplicate { id }
        );

        let row: (String, Vec<u8>, [u8; 12], [u8; 32]) = sqlx::query_as(
            "SELECT classification, bytes, hlc, author FROM event_log WHERE board = ? AND id = ?",
        )
        .bind(board.as_bytes().as_slice())
        .bind(id.as_bytes().as_slice())
        .fetch_one(&mut *store.connection.lock().await)
        .await
        .unwrap();
        assert_eq!(
            row,
            (
                class.into(),
                raw,
                original.hlc().to_key(),
                *original.author().as_bytes()
            )
        );
    }
    assert_eq!(store.latest_hlc(board).await.unwrap(), Some(original.hlc()));
}

#[tokio::test]
async fn duplicates_are_board_scoped_and_corrupt_bytes_are_rejected() {
    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([19; 32]);
    let other = BoardId::from_bytes([33; 32]);
    let bytes = creation().encode().unwrap();
    let id = EventId::from_bytes([78; 32]);
    let trusted = IdSource::Trusted(id);
    assert_eq!(
        store.append(board, &bytes, trusted).await.unwrap(),
        AppendOutcome::Applied {
            id,
            task: TaskId(id)
        }
    );
    assert_eq!(
        store.append(board, &bytes, trusted).await.unwrap(),
        AppendOutcome::Duplicate { id }
    );
    assert_eq!(
        store.append(other, &bytes, trusted).await.unwrap(),
        AppendOutcome::Applied {
            id,
            task: TaskId(id)
        }
    );
    assert!(matches!(
        store
            .append(board, &bytes[..bytes.len() - 1], trusted)
            .await,
        Err(StoreError::Decode(_))
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM event_log")
        .fetch_one(&mut *store.connection.lock().await)
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn rebuild_redecodes_skips_and_retains_trusted_ids() {
    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([19; 32]);
    let other = BoardId::from_bytes([52; 32]);
    let original = creation();
    let bytes = original.encode().unwrap();
    let id = EventId::from_bytes([0; 32]);
    for board in [board, other] {
        store
            .append(board, &bytes, IdSource::Trusted(id))
            .await
            .unwrap();
    }

    // Model an earlier decoder that stored this frame without recognizing the event.
    sqlx::query(
        "UPDATE event_log SET classification = 'unknown_event', hlc = NULL, author = NULL
                 WHERE board = ?",
    )
    .bind(board.as_bytes().as_slice())
    .execute(&mut *store.connection.lock().await)
    .await
    .unwrap();
    let future = [0x82, 2, 0xf6];
    let outcome = store
        .append(board, &future, IdSource::Content)
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        AppendOutcome::Skipped {
            reason: SkipReason::UnknownVersion(2),
            ..
        }
    ));

    assert_eq!(store.rebuild(board).await.unwrap().skipped, 1);
    let task = store.task(board, TaskId(id)).await.unwrap().unwrap();
    assert_eq!(store.tasks(other).await.unwrap(), [task]);
    let history = store.events(board, TaskId(id)).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, id);
    assert_eq!(history[0].bytes, bytes);
    assert_eq!(store.latest_hlc(board).await.unwrap(), Some(original.hlc()));
    let classes: Vec<String> =
        sqlx::query_scalar("SELECT classification FROM event_log WHERE board = ? ORDER BY id")
            .bind(board.as_bytes().as_slice())
            .fetch_all(&mut *store.connection.lock().await)
            .await
            .unwrap();
    assert_eq!(classes, ["known", "unknown_version"]);
}

#[tokio::test]
async fn rebuild_failure_leaves_the_current_view_intact() {
    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([19; 32]);
    let original = creation();
    let bytes = original.encode().unwrap();
    store
        .append(board, &bytes, IdSource::Content)
        .await
        .unwrap();
    let task = original.created_task().unwrap();
    let before = store.tasks(board).await.unwrap();
    let history = store.events(board, task).await.unwrap();
    sqlx::query(
        "INSERT INTO event_log (board, id, classification, bytes)
                 VALUES (?, ?, 'unknown_version', x'ff')",
    )
    .bind(board.as_bytes().as_slice())
    .bind([255_u8; 32])
    .execute(&mut *store.connection.lock().await)
    .await
    .unwrap();

    assert!(matches!(
        store.rebuild(board).await,
        Err(StoreError::Decode(_))
    ));
    assert_eq!(store.tasks(board).await.unwrap(), before);
    assert_eq!(store.events(board, task).await.unwrap(), history);
}

#[tokio::test]
async fn reopening_upgrades_every_boards_projection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pepino.sqlite");
    let envelope = creation();
    let task = envelope.created_task().unwrap();
    let bytes = envelope.encode().unwrap();
    let boards = [BoardId::from_bytes([19; 32]), BoardId::from_bytes([52; 32])];

    let older = Store::open_file(&path, crate::FOLD_VERSION).await.unwrap();
    for board in boards {
        older
            .append(board, &bytes, IdSource::Content)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM task_state")
        .execute(&mut *older.connection.lock().await)
        .await
        .unwrap();
    drop(older);
    let same_version = Store::open(&path).await.unwrap();
    assert_eq!(same_version.task(boards[0], task).await.unwrap(), None);
    drop(same_version);

    let version = crate::FOLD_VERSION + 1;
    let upgraded = Store::open_file(&path, version).await.unwrap();
    for board in boards {
        let record = upgraded.task(board, task).await.unwrap().unwrap();
        assert_eq!(record.title, "Replace the greenhouse latch");
    }
    let current: i64 = sqlx::query_scalar("SELECT version FROM fold_version")
        .fetch_one(&mut *upgraded.connection.lock().await)
        .await
        .unwrap();
    assert_eq!(current, version);
}

#[tokio::test]
async fn file_reads_see_the_committed_view_during_rebuild() {
    use std::time::Duration;

    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("pepino.sqlite"))
        .await
        .unwrap();
    let board = BoardId::from_bytes([19; 32]);
    let envelope = creation();
    let task = envelope.created_task().unwrap();
    store
        .append(board, &envelope.encode().unwrap(), IdSource::Content)
        .await
        .unwrap();

    let mut writer = store.connection.lock().await;
    sqlx::query("DELETE FROM task_state")
        .execute(&mut *writer)
        .await
        .unwrap();
    let mut tx = writer.begin().await.unwrap();
    rebuild(&mut tx, board).await.unwrap();

    let visible = tokio::time::timeout(Duration::from_secs(2), store.task(board, task))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(visible, None);
    tx.commit().await.unwrap();
    assert_eq!(
        store.task(board, task).await.unwrap().unwrap().title,
        "Replace the greenhouse latch"
    );
}

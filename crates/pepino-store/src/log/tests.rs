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

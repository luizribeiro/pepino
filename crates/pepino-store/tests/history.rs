use pepino_core::{
    BoardId, DeviceId, EdgeKind, Envelope, Event, EventId, Hlc, InitialFields, TaskId,
};
use pepino_store::{AppendOutcome, EventRecord, IdSource, Store};

#[tokio::test]
async fn history_includes_creation_and_both_edge_endpoints_in_envelope_order() {
    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([19; 32]);
    let task = TaskId::from_bytes([44; 32]);
    let to = TaskId::from_bytes([96; 32]);
    let hlc = Hlc {
        wall_ms: 1_795_144_800_000,
        counter: 3,
    };
    let mut expected = Vec::new();
    for (id, author, event) in [
        (56, 31, Event::TaskDeleted { task }),
        (
            48,
            55,
            Event::EdgeAdded {
                from: task,
                kind: EdgeKind::Blocks,
                to,
            },
        ),
        (
            44,
            31,
            Event::TaskCreated {
                title: "Paint the hallway".into(),
                fields: InitialFields::default(),
            },
        ),
        (
            36,
            79,
            Event::EdgeRemoved {
                from: task,
                kind: EdgeKind::Blocks,
                to,
            },
        ),
    ] {
        let id = EventId::from_bytes([id; 32]);
        let author = DeviceId::from_bytes([author; 32]);
        let bytes = Envelope::new(author, hlc, event).unwrap().encode().unwrap();
        let outcome = store
            .append(board, &bytes, IdSource::Trusted(id))
            .await
            .unwrap();
        assert_eq!(outcome, AppendOutcome::Applied { id, task });
        expected.push(EventRecord {
            id,
            author,
            hlc,
            bytes,
        });
    }
    assert_eq!(store.rebuild(board).await.unwrap().skipped, 0);
    assert!(store.tasks(board).await.unwrap().is_empty());
    assert!(
        store
            .events(BoardId::from_bytes([23; 32]), task)
            .await
            .unwrap()
            .is_empty()
    );
    expected.sort_by_key(|record| (record.hlc, record.id));
    assert_eq!(store.events(board, task).await.unwrap(), expected);
    assert_eq!(
        store.events(board, to).await.unwrap(),
        [expected[0].clone(), expected[2].clone()]
    );
}

#[tokio::test]
async fn board_history_keeps_skips_and_limits_after_task_filtering() {
    use ciborium::Value;
    use pepino_store::EventClassification::*;

    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([19; 32]);
    let author = DeviceId::from_bytes([51; 32]);
    let hlc = Hlc {
        wall_ms: 1_795_144_800_000,
        counter: 3,
    };
    let task = TaskId::from_bytes([2; 32]);
    let comment = Event::CommentAdded {
        task,
        text: "Use the longer hinge screws".into(),
    };
    let bytes = Envelope::new(author, hlc, comment.clone())
        .unwrap()
        .encode()
        .unwrap();
    let mut unknown: Value = ciborium::from_reader(bytes.as_slice()).unwrap();
    let content = unknown.as_array_mut().unwrap()[1].as_map_mut().unwrap();
    let (_, event) = content
        .iter_mut()
        .find(|(key, _)| key.as_text() == Some("event"))
        .unwrap();
    event.as_map_mut().unwrap()[0].0 = Value::from("reminder_added");
    let mut unknown_bytes = Vec::new();
    ciborium::into_writer(&unknown, &mut unknown_bytes).unwrap();
    for (byte, raw) in [
        (3, unknown_bytes.as_slice()),
        (1, &[0x82, 2, 0xf6]),
        (2, &bytes),
    ] {
        store
            .append(
                board,
                raw,
                IdSource::Trusted(EventId::from_bytes([byte; 32])),
            )
            .await
            .unwrap();
    }
    let history = store.history(board, None, None).await.unwrap();
    assert_eq!(
        history
            .iter()
            .map(|item| item.classification)
            .collect::<Vec<_>>(),
        [UnknownVersion, Applied, UnknownEvent]
    );
    assert_eq!(history[0].decoded.hlc(), None);
    assert_eq!(history[0].decoded.author(), None);
    assert_eq!(history[1].event, Some(comment));
    assert_eq!(history[2].decoded.author(), Some(author));
    assert_eq!(history[2].decoded.hlc(), Some(hlc));
    assert_eq!(history[2].tasks, [task]);
    assert_eq!(
        store.history(board, None, Some(2)).await.unwrap(),
        history[1..]
    );
    assert_eq!(
        store.history(board, Some(task), Some(1)).await.unwrap(),
        history[2..]
    );
    assert!(
        store
            .history(board, None, Some(0))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.events(board, task).await.unwrap().len(), 2);
}

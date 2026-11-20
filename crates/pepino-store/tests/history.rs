use pepino_core::{BoardId, DeviceId, EdgeKind, Envelope, Event, Hlc, InitialFields, TaskId};
use pepino_store::{AppendOutcome, EventRecord, Store};

#[tokio::test]
async fn history_includes_creation_and_both_edge_endpoints_in_envelope_order() {
    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([19; 32]);
    let to = TaskId::from_bytes([96; 32]);
    let hlc = Hlc {
        wall_ms: 1_795_144_800_000,
        counter: 3,
    };
    let creation = Event::TaskCreated {
        title: "Paint the hallway".into(),
        fields: InitialFields::default(),
    };
    let task = Envelope::new(DeviceId::from_bytes([31; 32]), hlc, creation.clone())
        .unwrap()
        .created_task()
        .unwrap();
    let mut expected = Vec::new();
    let mut edge_ids = Vec::new();
    for (author, event) in [
        (31, Event::TaskDeleted { task }),
        (
            55,
            Event::EdgeAdded {
                from: task,
                kind: EdgeKind::Blocks,
                to,
            },
        ),
        (31, creation),
        (
            79,
            Event::EdgeRemoved {
                from: task,
                kind: EdgeKind::Blocks,
                to,
            },
        ),
    ] {
        let author = DeviceId::from_bytes([author; 32]);
        let edge = matches!(event, Event::EdgeAdded { .. } | Event::EdgeRemoved { .. });
        let envelope = Envelope::new(author, hlc, event).unwrap();
        let id = envelope.id();
        if edge {
            edge_ids.push(id);
        }
        let bytes = envelope.encode().unwrap();
        let outcome = store.append(board, &bytes, None).await.unwrap();
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
    assert_eq!(
        store.task_titles(board, &[task, to]).await.unwrap(),
        [(task, "Paint the hallway".into())].into()
    );
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
        expected
            .into_iter()
            .filter(|record| edge_ids.contains(&record.id))
            .collect::<Vec<_>>()
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
    for raw in [unknown_bytes.as_slice(), &[0x82, 2, 0xf6], &bytes] {
        store.append(board, raw, None).await.unwrap();
    }
    let history = store.history(board, None, None).await.unwrap();
    let events = &history.events;
    assert!(events[0].decoded.id() < events[1].decoded.id());
    let applied = events
        .iter()
        .find(|item| item.classification == Applied)
        .unwrap();
    let unknown = events
        .iter()
        .find(|item| item.classification == UnknownEvent)
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(history.unordered[0].decoded.hlc(), None);
    assert_eq!(history.unordered[0].decoded.author(), None);
    assert_eq!(applied.event, Some(comment));
    assert_eq!(unknown.decoded.author(), Some(author));
    assert_eq!(unknown.decoded.hlc(), Some(hlc));
    assert_eq!(unknown.tasks, [task]);
    assert_eq!(store.history(board, None, Some(2)).await.unwrap(), history);
    assert_eq!(
        store
            .history(board, Some(task), Some(1))
            .await
            .unwrap()
            .events,
        events[1..]
    );
    assert!(
        store
            .history(board, None, Some(0))
            .await
            .unwrap()
            .events
            .is_empty()
    );
    assert!(
        store
            .history(board, Some(TaskId::from_bytes([3; 32])), None)
            .await
            .unwrap()
            .events
            .is_empty()
    );
    assert_eq!(
        store.history(board, None, Some(0)).await.unwrap().unordered,
        history.unordered
    );
    let replay = store.events(board, task).await.unwrap();
    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].bytes, bytes);
}

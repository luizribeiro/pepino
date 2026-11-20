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

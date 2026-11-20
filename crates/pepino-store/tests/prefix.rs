use pepino_core::{BoardId, DeviceId, Envelope, Event, EventId, Hlc, InitialFields, TaskId};
use pepino_store::{IdSource, PrefixResolution, Store, StoreError, TaskPrefixResolution};

#[tokio::test]
async fn prefix_matches_only_live_tasks_in_the_requested_board() {
    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([19; 32]);
    let other = BoardId::from_bytes([50; 32]);
    let author = DeviceId::from_bytes([33; 32]);
    let hlc = Hlc {
        wall_ms: 1_795_144_800_000,
        counter: 0,
    };
    let first = task_id(1);
    let second = task_id(2);
    for (target, id, title) in [
        (board, second, "Inspect the smoke alarm"),
        (other, first, "Label the spare keys"),
        (board, first, "Book the boiler service"),
    ] {
        let envelope = Envelope::new(
            author,
            hlc,
            Event::TaskCreated {
                title: title.into(),
                fields: InitialFields::default(),
            },
        )
        .unwrap();
        store
            .append(target, &envelope.encode().unwrap(), IdSource::Trusted(id.0))
            .await
            .unwrap();
    }
    assert_eq!(
        store.resolve_prefix(board, "bbbb").await.unwrap(),
        PrefixResolution::None
    );
    assert_eq!(
        store.resolve_prefix(board, "A1A101").await.unwrap(),
        PrefixResolution::One(first)
    );
    assert_eq!(
        store
            .resolve_prefix(board, &second.to_string())
            .await
            .unwrap(),
        PrefixResolution::One(second)
    );
    assert_eq!(
        store.resolve_prefix(board, "a1a1").await.unwrap(),
        PrefixResolution::Ambiguous {
            count: 2,
            candidates: vec![first, second]
        }
    );
    for invalid in ["", "a1a", "a1a_", &"a".repeat(65)] {
        assert!(matches!(
            store.resolve_prefix(board, invalid).await,
            Err(StoreError::InvalidPrefix)
        ));
    }
    let snapshot = store.board(board).await.unwrap();
    let delete = Envelope::new(author, hlc, Event::TaskDeleted { task: first }).unwrap();
    store
        .append(
            board,
            &delete.encode().unwrap(),
            IdSource::Trusted(EventId::from_bytes([99; 32])),
        )
        .await
        .unwrap();
    assert_eq!(
        store.resolve_prefix(board, "a1a1").await.unwrap(),
        PrefixResolution::One(second)
    );
    assert_eq!(
        store.resolve_prefix(other, "a1a1").await.unwrap(),
        PrefixResolution::One(first)
    );
    let view = store.board(board).await.unwrap();
    assert_eq!(
        view.resolve_task_prefix("A1A1").unwrap(),
        TaskPrefixResolution::Live(second)
    );
    assert_eq!(
        view.resolve_task_prefix("a1a101").unwrap(),
        TaskPrefixResolution::Deleted { id: Some(first) }
    );
    assert_eq!(
        view.resolve_task_prefix("a1a103").unwrap(),
        TaskPrefixResolution::Unknown
    );
    assert_eq!(
        snapshot.resolve_task_prefix("a1a1").unwrap(),
        TaskPrefixResolution::Ambiguous {
            count: 2,
            candidates: vec![first, second],
        }
    );

    let delete = Envelope::new(author, hlc, Event::TaskDeleted { task: second }).unwrap();
    store
        .append(board, &delete.encode().unwrap(), IdSource::Content)
        .await
        .unwrap();
    assert_eq!(
        store
            .board(board)
            .await
            .unwrap()
            .resolve_task_prefix("a1a1")
            .unwrap(),
        TaskPrefixResolution::Deleted { id: None },
    );
}

fn task_id(suffix: u8) -> TaskId {
    let mut bytes = [suffix; 32];
    bytes[..2].fill(0xa1);
    TaskId::from_bytes(bytes)
}

#[tokio::test]
async fn ambiguous_prefix_returns_only_the_first_ten_of_all_matches() {
    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([19; 32]);
    for suffix in (1..=14).rev() {
        let bytes = Envelope::new(
            DeviceId::from_bytes([33; 32]),
            Hlc {
                wall_ms: 1_795_144_800_000,
                counter: 0,
            },
            Event::TaskCreated {
                title: format!("Inspect smoke alarm {suffix}"),
                fields: InitialFields::default(),
            },
        )
        .unwrap()
        .encode()
        .unwrap();
        store
            .append(board, &bytes, IdSource::Trusted(task_id(suffix).0))
            .await
            .unwrap();
    }
    assert_eq!(
        store.resolve_prefix(board, "a1a1").await.unwrap(),
        PrefixResolution::Ambiguous {
            count: 14,
            candidates: (1..=10).map(task_id).collect()
        }
    );
}

#[tokio::test]
async fn a_delete_that_arrives_before_creation_resolves_as_deleted() {
    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([19; 32]);
    let task = task_id(3);
    let delete = Envelope::new(
        DeviceId::from_bytes([33; 32]),
        Hlc {
            wall_ms: 1_795_144_800_000,
            counter: 0,
        },
        Event::TaskDeleted { task },
    )
    .unwrap();
    store
        .append(board, &delete.encode().unwrap(), IdSource::Content)
        .await
        .unwrap();

    let view = store.board(board).await.unwrap();
    assert_eq!(
        view.resolve_prefix("a1a103").unwrap(),
        PrefixResolution::None
    );
    assert_eq!(
        view.resolve_task_prefix("a1a103").unwrap(),
        TaskPrefixResolution::Deleted { id: Some(task) }
    );
    assert_eq!(
        view.resolve_task_prefix("a1a104").unwrap(),
        TaskPrefixResolution::Unknown
    );
}

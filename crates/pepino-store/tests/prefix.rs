use pepino_core::{BoardId, DeviceId, Envelope, Event, EventId, Hlc, InitialFields, TaskId};
use pepino_store::{IdSource, PrefixResolution, Store, StoreError};

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
    let first = TaskId::from_bytes([0xa1; 32]);
    let second = TaskId::from_bytes([0xa2; 32]);
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
        store.resolve_prefix(board, "b").await.unwrap(),
        PrefixResolution::None
    );
    assert_eq!(
        store.resolve_prefix(board, "A1A").await.unwrap(),
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
        store.resolve_prefix(board, "a").await.unwrap(),
        PrefixResolution::Ambiguous(vec![first, second])
    );
    for invalid in ["", "a_", &"a".repeat(65)] {
        assert!(matches!(
            store.resolve_prefix(board, invalid).await,
            Err(StoreError::InvalidPrefix)
        ));
    }
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
        store.resolve_prefix(board, "a").await.unwrap(),
        PrefixResolution::One(second)
    );
    assert_eq!(
        store.resolve_prefix(other, "a").await.unwrap(),
        PrefixResolution::One(first)
    );
}

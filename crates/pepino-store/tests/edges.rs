use pepino_core::{
    BoardId, DeviceId, EdgeKind, Envelope, Event, EventId, Hlc, InitialFields, TaskId,
};
use pepino_store::{EdgeExclusion, EdgeRecord, IdSource, Store};

const BOARD: BoardId = BoardId::from_bytes([29; 32]);

async fn append(store: &Store, id: u8, tick: u32, event: Event) {
    let envelope = Envelope::with_id(
        EventId::from_bytes([id; 32]),
        DeviceId::from_bytes([40 + id; 32]),
        Hlc {
            wall_ms: 1_795_144_800_000,
            counter: tick,
        },
        event,
    )
    .unwrap();
    store
        .append(
            BOARD,
            &envelope.encode().unwrap(),
            IdSource::Trusted(envelope.id()),
        )
        .await
        .unwrap();
}

async fn add(store: &Store, id: u8, tick: u32, from: TaskId, kind: EdgeKind, to: TaskId) {
    append(store, id, tick, Event::EdgeAdded { from, kind, to }).await;
}

async fn setup() -> (Store, [TaskId; 4]) {
    let store = Store::open_in_memory().await.unwrap();
    let titles = [
        "Prepare the greenhouse",
        "Repair the door",
        "Order brass hinges",
        "Paint the frame",
    ];
    for (index, title) in titles.into_iter().enumerate() {
        append(
            &store,
            index as u8 + 1,
            0,
            Event::TaskCreated {
                title: title.into(),
                fields: InitialFields::default(),
            },
        )
        .await;
    }
    (store, [1, 2, 3, 4].map(|id| TaskId::from_bytes([id; 32])))
}

#[tokio::test]
async fn newest_live_parent_wins_and_previous_parent_can_return() {
    let (store, [greenhouse, door, _, frame]) = setup().await;
    add(&store, 12, 2, frame, EdgeKind::Parent, door).await;
    add(&store, 11, 2, frame, EdgeKind::Parent, greenhouse).await;
    let older = EdgeRecord {
        from: frame,
        kind: EdgeKind::Parent,
        to: greenhouse,
        exclusion: None,
    };
    let newer = EdgeRecord { to: door, ..older };
    assert_eq!(
        store.edges(BOARD, frame).await.unwrap(),
        [
            EdgeRecord {
                exclusion: Some(EdgeExclusion::NewerParent),
                ..older
            },
            newer,
        ]
    );
    append(&store, 13, 3, Event::TaskDeleted { task: door }).await;
    assert_eq!(
        store.edges(BOARD, frame).await.unwrap(),
        [
            older,
            EdgeRecord {
                exclusion: Some(EdgeExclusion::MissingEndpoint),
                ..newer
            },
        ]
    );
}

#[tokio::test]
async fn removing_an_edge_revives_the_loser_of_a_concurrent_two_task_cycle() {
    let (store, [greenhouse, door, _, _]) = setup().await;
    add(&store, 12, 2, greenhouse, EdgeKind::Parent, door).await;
    add(&store, 11, 2, door, EdgeKind::Parent, greenhouse).await;
    let winner = EdgeRecord {
        from: door,
        kind: EdgeKind::Parent,
        to: greenhouse,
        exclusion: None,
    };
    let loser = EdgeRecord {
        from: greenhouse,
        to: door,
        ..winner
    };
    assert_eq!(
        store.edges(BOARD, door).await.unwrap(),
        [
            winner,
            EdgeRecord {
                exclusion: Some(EdgeExclusion::Cycle),
                ..loser
            },
        ]
    );
    append(
        &store,
        13,
        3,
        Event::EdgeRemoved {
            from: door,
            kind: EdgeKind::Parent,
            to: greenhouse,
        },
    )
    .await;
    assert_eq!(store.edges(BOARD, door).await.unwrap(), [loser]);
}

#[tokio::test]
async fn deleting_a_task_revives_the_loser_of_a_concurrent_three_task_cycle() {
    let (store, [_, door, hinges, frame]) = setup().await;
    add(&store, 13, 2, frame, EdgeKind::Blocks, hinges).await;
    add(&store, 11, 2, hinges, EdgeKind::Blocks, door).await;
    add(&store, 12, 2, door, EdgeKind::Blocks, frame).await;
    let winner = EdgeRecord {
        from: door,
        kind: EdgeKind::Blocks,
        to: frame,
        exclusion: None,
    };
    let loser = EdgeRecord {
        from: frame,
        to: hinges,
        ..winner
    };
    assert_eq!(
        store.edges(BOARD, frame).await.unwrap(),
        [
            winner,
            EdgeRecord {
                exclusion: Some(EdgeExclusion::Cycle),
                ..loser
            },
        ]
    );
    append(&store, 14, 3, Event::TaskDeleted { task: door }).await;
    assert_eq!(
        store.edges(BOARD, frame).await.unwrap(),
        [
            EdgeRecord {
                exclusion: Some(EdgeExclusion::MissingEndpoint),
                ..winner
            },
            loser,
        ]
    );
}

#[tokio::test]
async fn symmetric_edges_share_presence_and_removal_in_either_spelling() {
    let (store, [_, door, hinges, _]) = setup().await;
    let relation = EdgeRecord {
        from: door,
        kind: EdgeKind::Relates,
        to: hinges,
        exclusion: None,
    };
    add(&store, 11, 2, hinges, EdgeKind::Relates, door).await;
    add(&store, 12, 3, door, EdgeKind::Relates, hinges).await;
    assert_eq!(store.edges(BOARD, door).await.unwrap(), [relation]);
    assert_eq!(store.edges(BOARD, hinges).await.unwrap(), [relation]);
    append(
        &store,
        13,
        4,
        Event::EdgeRemoved {
            from: hinges,
            kind: EdgeKind::Relates,
            to: door,
        },
    )
    .await;
    add(&store, 14, 3, door, EdgeKind::Relates, hinges).await;
    assert!(store.edges(BOARD, door).await.unwrap().is_empty());
}

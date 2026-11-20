use crate::{Dependencies, Dependency, EdgeExclusion, EdgeRecord, IdSource, Store, SubtreeEntry};
use pepino_core::{
    BoardId, DeviceId, EdgeKind, Envelope, Event, FieldName, FieldValue, Hlc, InitialFields,
    Status, TaskId,
};

const BOARD: BoardId = BoardId::from_bytes([29; 32]);

#[tokio::test]
async fn dependencies_use_shortest_distances_in_both_directions() {
    let (store, [greenhouse, door, hinges, frame]) = setup().await;
    for (id, from, to) in [
        (11, hinges, door),
        (12, hinges, frame),
        (13, door, frame),
        (14, frame, greenhouse),
    ] {
        add(&store, id, u32::from(id), from, EdgeKind::Blocks, to).await;
    }
    add(&store, 15, 15, greenhouse, EdgeKind::Blocks, hinges).await;
    let view = store.board(BOARD).await.unwrap();
    let at = |task, distance| Dependency { task, distance };
    assert_eq!(
        view.dependencies(frame),
        Dependencies {
            blocked_by: vec![at(door, 1), at(hinges, 1)],
            blocks: vec![at(greenhouse, 1)],
        }
    );
    assert_eq!(
        view.dependencies(hinges),
        Dependencies {
            blocked_by: vec![],
            blocks: vec![at(door, 1), at(frame, 1), at(greenhouse, 2)],
        }
    );
    assert_eq!(
        view.dependencies(greenhouse),
        Dependencies {
            blocked_by: vec![at(frame, 1), at(door, 2), at(hinges, 2)],
            blocks: vec![],
        }
    );
    assert_eq!(
        view.dependencies(TaskId::from_bytes([99; 32])),
        Dependencies::default()
    );
}

#[tokio::test]
async fn subtree_lists_effective_descendants_in_stable_preorder() {
    let (store, [greenhouse, door, hinges, frame]) = setup().await;
    let entry = |task, depth| SubtreeEntry { task, depth };
    assert_eq!(
        store.subtree(BOARD, greenhouse).await.unwrap(),
        [entry(greenhouse, 0)]
    );
    add(&store, 11, 1, hinges, EdgeKind::Parent, greenhouse).await;
    add(&store, 12, 2, door, EdgeKind::Parent, greenhouse).await;
    add(&store, 13, 3, frame, EdgeKind::Parent, door).await;
    let view = store.board(BOARD).await.unwrap();
    assert_eq!(
        view.subtree(greenhouse),
        [
            entry(greenhouse, 0),
            entry(door, 1),
            entry(frame, 2),
            entry(hinges, 1),
        ]
    );
    let records = view.tasks();
    assert_eq!(records[0].children, [door, hinges].into());
    assert_eq!(records[3].parent, Some(door));
    append(&store, 14, 4, Event::TaskDeleted { task: door }).await;
    let updated = store.board(BOARD).await.unwrap();
    assert!(updated.subtree(door).is_empty());
    assert_eq!(view.subtree(door), [entry(door, 0), entry(frame, 1)]);
    assert_eq!(
        updated.subtree(greenhouse),
        [entry(greenhouse, 0), entry(hinges, 1),]
    );
}

fn ordered_envelope(id_prefix: u8, tick: u32, event: Event) -> Envelope {
    // Other graph fixtures refer to these events in first-byte hash order.
    (0_u32..)
        .map(|nonce| {
            let mut author = [40 + id_prefix; 32];
            author[..4].copy_from_slice(&nonce.to_le_bytes());
            Envelope::new(
                DeviceId::from_bytes(author),
                Hlc {
                    wall_ms: 1_795_144_800_000,
                    counter: tick,
                },
                event.clone(),
            )
            .unwrap()
        })
        .find(|envelope| envelope.id().as_bytes()[0] == id_prefix)
        .unwrap()
}

async fn append(store: &Store, id: u8, tick: u32, event: Event) -> Envelope {
    let envelope = ordered_envelope(id, tick, event);
    store
        .append(BOARD, &envelope.encode().unwrap(), IdSource::Content)
        .await
        .unwrap();
    envelope
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
    let mut ids = Vec::new();
    for (index, title) in titles.into_iter().enumerate() {
        let creation = append(
            &store,
            index as u8 + 1,
            0,
            Event::TaskCreated {
                title: title.into(),
                fields: InitialFields::default(),
            },
        )
        .await;
        ids.push(creation.created_task().unwrap());
    }
    (store, ids.try_into().unwrap())
}

#[tokio::test]
async fn newest_parent_wins_even_when_its_target_is_deleted() {
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
            EdgeRecord {
                exclusion: Some(EdgeExclusion::NewerParent),
                ..older
            },
            EdgeRecord {
                exclusion: Some(EdgeExclusion::MissingEndpoint),
                ..newer
            },
        ]
    );
    assert_eq!(
        store.task(BOARD, frame).await.unwrap().unwrap().parent,
        None
    );
}

#[tokio::test]
async fn newest_parent_with_a_missing_target_revives_when_created() {
    let (store, [greenhouse, _, _, frame]) = setup().await;
    let creation = Event::TaskCreated {
        title: "Weatherproof the garden shed".into(),
        fields: InitialFields::default(),
    };
    let shed = ordered_envelope(5, 3, creation.clone())
        .created_task()
        .unwrap();
    add(&store, 11, 1, frame, EdgeKind::Parent, greenhouse).await;
    add(&store, 12, 2, frame, EdgeKind::Parent, shed).await;

    assert_eq!(
        store.task(BOARD, frame).await.unwrap().unwrap().parent,
        None
    );
    let edges = store.edges(BOARD, frame).await.unwrap();
    assert_eq!(edges[0].exclusion, Some(EdgeExclusion::NewerParent));
    assert_eq!(edges[1].exclusion, Some(EdgeExclusion::MissingEndpoint));

    append(&store, 5, 3, creation).await;
    assert_eq!(
        store.task(BOARD, frame).await.unwrap().unwrap().parent,
        Some(shed)
    );
    assert!(store.edges(BOARD, frame).await.unwrap()[1].is_effective());
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
    for (task, other) in [(door, hinges), (hinges, door)] {
        assert_eq!(
            store.task(BOARD, task).await.unwrap().unwrap().relates,
            [other].into()
        );
    }
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

#[tokio::test]
async fn readiness_follows_status_and_live_blockers() {
    let (store, [_, door, hinges, _]) = setup().await;
    add(&store, 10, 1, hinges, EdgeKind::Blocks, door).await;
    let status = |task, status| Event::TaskFieldSet {
        task,
        field: FieldName::new("status").unwrap(),
        value: FieldValue::Status(status),
    };
    let assert_state = async |blocked, ready| {
        let task = store.task(BOARD, door).await.unwrap().unwrap();
        assert_eq!((task.blocked, task.ready), (blocked, ready));
    };
    assert_state(true, false).await;
    assert_eq!(
        store.task(BOARD, door).await.unwrap().unwrap().blocked_by,
        [hinges].into()
    );
    assert_eq!(
        store.task(BOARD, hinges).await.unwrap().unwrap().blocks,
        [door].into()
    );
    for (id, blocker_status, blocked) in [
        (11, Status::Done, false),
        (12, Status::Todo, true),
        (13, Status::Cancelled, false),
        (14, Status::Doing, true),
    ] {
        append(&store, id, u32::from(id), status(hinges, blocker_status)).await;
        assert_state(blocked, !blocked).await;
    }
    append(&store, 15, 15, status(door, Status::Done)).await;
    assert_state(false, false).await;
    append(&store, 16, 16, status(door, Status::Todo)).await;
    assert_state(true, false).await;
    append(&store, 17, 17, Event::TaskDeleted { task: hinges }).await;
    assert_state(false, true).await;
    append(&store, 18, 18, status(door, Status::Doing)).await;
    assert_state(false, false).await;
    assert!(
        store
            .task(BOARD, door)
            .await
            .unwrap()
            .unwrap()
            .blocked_by
            .is_empty()
    );
}

#[tokio::test]
async fn loads_long_chains_in_adversarial_register_order() {
    use std::time::{Duration, Instant};

    let store = Store::open_in_memory().await.unwrap();
    let mut tasks = Vec::new();
    let write = async |counter, event| {
        let envelope = Envelope::new(
            DeviceId::from_bytes([51 + (counter % 3) as u8; 32]),
            Hlc {
                wall_ms: 1_795_144_800_000,
                counter,
            },
            event,
        )
        .unwrap();
        store
            .append(BOARD, &envelope.encode().unwrap(), IdSource::Content)
            .await
            .unwrap();
        envelope
    };
    for panel in 0..5000 {
        let envelope = write(
            0,
            Event::TaskCreated {
                title: format!("Inspect greenhouse panel {}", panel + 1),
                fields: InitialFields::default(),
            },
        )
        .await;
        tasks.push(envelope.created_task().unwrap());
    }
    for kind in [EdgeKind::Parent, EdgeKind::Blocks] {
        for (index, pair) in tasks.windows(2).rev().enumerate() {
            write(
                index as u32 + 1,
                Event::EdgeAdded {
                    from: pair[0],
                    kind,
                    to: pair[1],
                },
            )
            .await;
        }
        write(
            5000,
            Event::EdgeAdded {
                from: tasks[4999],
                kind,
                to: tasks[0],
            },
        )
        .await;
    }

    let start = Instant::now();
    let view = store.board(BOARD).await.unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "loading a board with two 5,000-task chains took {elapsed:?}"
    );
    assert_eq!(view.tasks().len(), 5000);
    assert_eq!(view.subtree(tasks[4999]).last().unwrap().depth, 4999);
    assert_eq!(
        view.dependencies(tasks[0]).blocks.last().unwrap().distance,
        4999
    );
    let excluded: Vec<_> = view
        .edges(tasks[0])
        .into_iter()
        .filter_map(|edge| edge.exclusion)
        .collect();
    assert_eq!(excluded, [EdgeExclusion::Cycle; 2]);
}

proptest::proptest! {
    #[test]
    fn a_simple_cycle_rejects_its_newest_edge(
        writes in proptest::array::uniform3((0..8u8, 1..8u32)),
    ) {
        use super::{EdgeRow, edge_key, project};

        for kind in [EdgeKind::Parent, EdgeKind::Blocks] {
            let mut ring = Vec::new();
            for (index, (author, counter)) in writes.into_iter().enumerate() {
                let from = TaskId::from_bytes([index as u8 + 1; 32]);
                let to = TaskId::from_bytes([((index + 1) % 3) as u8 + 1; 32]);
                let envelope = Envelope::new(
                    DeviceId::from_bytes([51 + author; 32]),
                    Hlc { wall_ms: 1_795_144_800_000, counter },
                    Event::EdgeAdded { from, kind, to },
                ).unwrap();
                ring.push((envelope.order_key(), EdgeRow {
                    task: *from.as_bytes(),
                    member: edge_key::member(kind, to).unwrap(),
                    live: true,
                    source_status: None,
                }));
            }
            ring.sort_unstable_by_key(|(key, _)| *key);
            let rows: Vec<_> = ring.into_iter().map(|(_, row)| row).collect();
            let exclusions: Vec<_> = project(&rows).unwrap().into_iter().map(|edge| edge.record.exclusion).collect();
            proptest::prop_assert_eq!(exclusions, [None, None, Some(EdgeExclusion::Cycle)]);
        }
    }
}

#[tokio::test]
async fn blocker_with_no_status_register_leaves_its_dependents_ready() {
    let (store, [_, door, hinges, _]) = setup().await;
    add(&store, 10, 1, hinges, EdgeKind::Blocks, door).await;
    assert!(
        store
            .board(BOARD)
            .await
            .unwrap()
            .task(door)
            .unwrap()
            .blocked
    );

    sqlx::query(
        "DELETE FROM task_register WHERE board = ? AND task = ? AND kind = 'field' AND member = ?",
    )
    .bind(crate::BoardKey(BOARD))
    .bind(hinges.as_bytes().as_slice())
    .bind(b"status".as_slice())
    .execute(&mut *store.connection.lock().await)
    .await
    .unwrap();

    let view = store.board(BOARD).await.unwrap();
    let door = view.task(door).unwrap();
    assert_eq!(door.blocked_by, [hinges].into());
    assert_eq!((door.blocked, door.ready), (false, true));
    assert!(view.edges(door.id)[0].is_effective());
}

#[tokio::test]
async fn siblings_follow_creation_even_when_ids_sort_in_reverse() {
    let (store, [greenhouse, _, _, _]) = setup().await;
    let mut ids = Vec::new();
    for (id, tick, title) in [(19, 3, "Fit the draft seal"), (25, 1, "Clean the frame")] {
        let creation = append(
            &store,
            id,
            tick,
            Event::TaskCreated {
                title: title.into(),
                fields: InitialFields::default(),
            },
        )
        .await;
        ids.push(creation.created_task().unwrap());
    }
    let [younger, older] = ids.try_into().unwrap();
    add(&store, 28, 4, younger, EdgeKind::Parent, greenhouse).await;
    add(&store, 29, 5, older, EdgeKind::Parent, greenhouse).await;
    let nodes = store.subtree(BOARD, greenhouse).await.unwrap();
    assert_eq!(
        nodes.iter().map(|entry| entry.task).collect::<Vec<_>>(),
        [greenhouse, older, younger]
    );
}

#[tokio::test]
async fn removing_a_parent_can_activate_an_older_candidate() {
    let (store, [greenhouse, door, _, frame]) = setup().await;
    add(&store, 11, 1, frame, EdgeKind::Parent, greenhouse).await;
    add(&store, 12, 2, frame, EdgeKind::Parent, door).await;
    let view = store.board(BOARD).await.unwrap();
    assert_eq!(
        view.edge_records()[0].exclusion,
        Some(EdgeExclusion::NewerParent)
    );
    assert_eq!(
        view.edges_after_removing(&[door].into()),
        [EdgeRecord {
            from: frame,
            kind: EdgeKind::Parent,
            to: greenhouse,
            exclusion: None
        }]
    );
    assert_eq!(view.task(frame).unwrap().parent, Some(door));
}

#[tokio::test]
async fn board_retains_known_assignee_ids_after_removal() {
    let (store, [greenhouse, _, _, _]) = setup().await;
    let who = pepino_core::IdentityId::from_bytes([87; 32]);
    append(
        &store,
        12,
        1,
        Event::AssigneeAdded {
            task: greenhouse,
            who,
        },
    )
    .await;
    append(
        &store,
        13,
        2,
        Event::AssigneeRemoved {
            task: greenhouse,
            who,
        },
    )
    .await;
    let view = store.board(BOARD).await.unwrap();
    assert!(view.task(greenhouse).unwrap().assignees.is_empty());
    assert_eq!(view.assignee_ids(), &[who].into());
}

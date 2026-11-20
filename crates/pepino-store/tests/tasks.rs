use pepino_core::{
    BoardId, Clock, Date, DeviceId, Envelope, Event, FieldName, FieldValue, Hlc, IdentityId,
    InitialFields, Status,
};
use pepino_store::{IdSource, Store, TaskRecord};

#[tokio::test]
async fn creation_merges_with_retained_registers_by_clock() {
    let board = BoardId::from_bytes([53; 32]);
    let author = DeviceId::from_bytes([41; 32]);
    let collaborator = DeviceId::from_bytes([61; 32]);
    let who = IdentityId::from_bytes([29; 32]);
    let mut creator_clock = Clock::new();
    let mut editor_clock = Clock::new();
    let wall_ms = 1_795_144_800_000;
    let store = Store::open_in_memory().await.unwrap();
    let custom = FieldName::new("supplier").unwrap();
    let due = Date::from_ymd_opt(2026, 11, 30).unwrap();
    let creation = Envelope::new(
        author,
        creator_clock.tick(wall_ms + 10).unwrap(),
        Event::TaskCreated {
            title: "Repair garden gate".into(),
            fields: InitialFields {
                description: Some("Replace the lower hinge".into()),
                status: Status::Doing,
                due: Some(due),
                priority: Some(2),
                tags: ["garden".into(), "weekend".into()].into(),
                assignees: [who].into(),
                custom: [(
                    custom.clone(),
                    FieldValue::Text("Oak Street Hardware".into()),
                )]
                .into(),
            },
        },
    )
    .unwrap();
    let task = creation.created_task().unwrap();
    let edits = [
        ("title", FieldValue::Text("Old gate plan".into()), 5),
        ("priority", FieldValue::Integer(99), 5),
        ("description", FieldValue::Unset, 20),
        (
            "title",
            FieldValue::Text("Fit new hinges to the garden gate".into()),
            20,
        ),
    ];
    for (field, value, millis) in edits {
        let edit = Envelope::new(
            collaborator,
            editor_clock.tick(wall_ms + millis).unwrap(),
            Event::TaskFieldSet {
                task,
                field: FieldName::new(field).unwrap(),
                value,
            },
        )
        .unwrap();
        store
            .append(board, &edit.encode().unwrap(), IdSource::Content)
            .await
            .unwrap();
    }
    assert_eq!(store.task(board, task).await.unwrap(), None);
    assert!(store.tasks(board).await.unwrap().is_empty());
    store
        .append(board, &creation.encode().unwrap(), IdSource::Content)
        .await
        .unwrap();
    let expected = TaskRecord {
        id: task,
        title: "Fit new hinges to the garden gate".into(),
        description: None,
        status: Status::Doing,
        due: Some(due),
        priority: Some(2),
        custom: [(custom, FieldValue::Text("Oak Street Hardware".into()))].into(),
        tags: ["garden".into(), "weekend".into()].into(),
        assignees: [who].into(),
        created: creation.hlc(),
        updated: Hlc {
            wall_ms: wall_ms + 20,
            counter: 1,
        },
        author,
        parent: None,
        children: Default::default(),
        blocks: Default::default(),
        blocked_by: Default::default(),
        relates: Default::default(),
        blocked: false,
        comment_count: 0,
        ready: false,
    };
    assert_eq!(
        store.task(board, task).await.unwrap(),
        Some(expected.clone())
    );
    assert_eq!(store.tasks(board).await.unwrap(), [expected]);
    assert!(
        store
            .tasks(BoardId::from_bytes([89; 32]))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn lists_tasks_by_full_creation_clock_then_id() {
    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([19; 32]);
    let author = DeviceId::from_bytes([33; 32]);
    let later = Hlc {
        wall_ms: 1 << 63,
        counter: 3,
    };
    let earlier = Hlc {
        counter: 2,
        ..later
    };
    let mut expected = Vec::new();
    for (hlc, title) in [
        (later, "Paint the porch"),
        (later, "Order the paint"),
        (earlier, "Measure the porch"),
    ] {
        let creation = Envelope::new(
            author,
            hlc,
            Event::TaskCreated {
                title: title.into(),
                fields: InitialFields::default(),
            },
        )
        .unwrap();
        expected.push((hlc, creation.created_task().unwrap()));
        store
            .append(board, &creation.encode().unwrap(), IdSource::Content)
            .await
            .unwrap();
    }
    expected.sort_unstable();
    let tasks = store.tasks(board).await.unwrap();
    assert_eq!(
        tasks
            .into_iter()
            .map(|task| (task.created, task.id))
            .collect::<Vec<_>>(),
        expected
    );
}

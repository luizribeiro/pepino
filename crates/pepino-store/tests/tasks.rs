use pepino_core::{
    BoardId, Clock, Date, DeviceId, Envelope, Event, FieldName, FieldValue, IdentityId,
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
        created: wall_ms + 10,
        updated: wall_ms + 20,
        author,
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

use pepino_core::{BoardId, DeviceId, Envelope, Event, EventId, Hlc, InitialFields};
use pepino_store::{CommentRecord, IdSource, Store};

#[tokio::test]
async fn comments_are_visible_only_on_live_tasks_and_ordered_by_clock_then_id() {
    let board = BoardId::from_bytes([19; 32]);
    let store = Store::open_in_memory().await.unwrap();
    let hlc = Hlc {
        wall_ms: 1_795_144_800_000,
        counter: 0,
    };
    let creation = Envelope::new(
        DeviceId::from_bytes([33; 32]),
        hlc,
        Event::TaskCreated {
            title: "Seal the balcony rail".into(),
            fields: InitialFields::default(),
        },
    )
    .unwrap();
    let task = creation.created_task().unwrap();
    let mut comments = Vec::new();
    for (id, author, counter, text) in [
        (14, 33, 2, "Bring painter’s tape"),
        (95, 54, 0, "Sand the rail first"),
        (10, 74, 2, "Paint is in the closet"),
    ] {
        let id = EventId::from_bytes([id; 32]);
        let author = DeviceId::from_bytes([author; 32]);
        let hlc = Hlc { counter, ..hlc };
        let envelope = Envelope::new(
            author,
            hlc,
            Event::CommentAdded {
                task,
                text: text.into(),
            },
        )
        .unwrap();
        store
            .append(board, &envelope.encode().unwrap(), IdSource::Trusted(id))
            .await
            .unwrap();
        comments.push(CommentRecord {
            id,
            author,
            hlc,
            text: text.into(),
        });
    }
    assert!(store.comments(board, task).await.unwrap().is_empty());
    store
        .append(board, &creation.encode().unwrap(), IdSource::Content)
        .await
        .unwrap();
    comments.sort_by_key(|comment| (comment.hlc, comment.id));
    assert_eq!(store.comments(board, task).await.unwrap(), comments);
    let deletion = Envelope::new(creation.author(), hlc, Event::TaskDeleted { task }).unwrap();
    store
        .append(board, &deletion.encode().unwrap(), IdSource::Content)
        .await
        .unwrap();
    assert!(store.comments(board, task).await.unwrap().is_empty());
    assert_eq!(store.task(board, task).await.unwrap(), None);
}

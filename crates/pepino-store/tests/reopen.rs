use pepino_core::{BoardId, DeviceId, Envelope, Event, Hlc, InitialFields};
use pepino_store::{AppendOutcome, IdSource, PrefixResolution, Store};

#[tokio::test]
async fn file_reopen_preserves_the_view_clock_and_log() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pepino.sqlite");
    let board = BoardId::from_bytes([81; 32]);
    let author = DeviceId::from_bytes([41; 32]);
    let clock = Hlc {
        wall_ms: 1_795_144_800_000,
        counter: 1,
    };
    let creation = Envelope::new(
        author,
        clock,
        Event::TaskCreated {
            title: "Book a chimney inspection".into(),
            fields: InitialFields {
                description: Some("Ask for the November appointment list".into()),
                tags: ["home".into()].into(),
                ..InitialFields::default()
            },
        },
    )
    .unwrap();
    let task = creation.created_task().unwrap();
    let creation_bytes = creation.encode().unwrap();
    let newer = Hlc {
        counter: 2,
        ..clock
    };
    let comment = Envelope::new(
        DeviceId::from_bytes([62; 32]),
        newer,
        Event::CommentAdded {
            task,
            text: "Inspector is available on Tuesday".into(),
        },
    )
    .unwrap();
    let (tasks, comments, history) = {
        let store = Store::open(&path).await.unwrap();
        store
            .append(board, &creation_bytes, IdSource::Content)
            .await
            .unwrap();
        store
            .append(board, &comment.encode().unwrap(), IdSource::Content)
            .await
            .unwrap();
        (
            store.tasks(board).await.unwrap(),
            store.comments(board, task).await.unwrap(),
            store.events(board, task).await.unwrap(),
        )
    };
    let reopened = Store::open(&path).await.unwrap();
    assert_eq!(reopened.tasks(board).await.unwrap(), tasks);
    assert_eq!(reopened.comments(board, task).await.unwrap(), comments);
    assert_eq!(reopened.events(board, task).await.unwrap(), history);
    assert_eq!(reopened.latest_hlc(board).await.unwrap(), Some(newer));
    assert_eq!(
        reopened
            .resolve_prefix(board, &task.to_string()[..9])
            .await
            .unwrap(),
        PrefixResolution::One(task)
    );
    assert_eq!(
        reopened
            .append(board, &creation_bytes, IdSource::Content)
            .await
            .unwrap(),
        AppendOutcome::Duplicate { id: creation.id() }
    );
}

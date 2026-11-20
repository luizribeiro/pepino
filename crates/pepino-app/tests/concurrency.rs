use pepino_app::{App, AppError, Request};

const NOW: u64 = 1_795_144_800_000;

#[tokio::test]
async fn open_and_handle_are_send() {
    fn assert_send<T: Send>(value: T) -> T {
        value
    }

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_owned();
    let error = tokio::spawn(async move {
        let mut app = assert_send(App::open(path, NOW)).await.unwrap();
        let request = Request::GetTask {
            task: "abcd".into(),
        };
        assert_send(app.handle(request, NOW)).await.unwrap_err()
    })
    .await
    .unwrap();
    assert!(matches!(error, AppError::NotFound { .. }));
}

#[tokio::test]
async fn concurrent_apps_open_and_write_to_the_same_directory() {
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::time::Duration;

    use pepino_app::{OpenOptions, Response};
    use serde_json::json;
    use tokio::sync::Barrier;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("greenhouse");
    let titles = [
        "Order a replacement latch",
        "Cut the weather stripping",
        "Inspect the hinges",
        "Seal the glazing",
        "Clean the gutters",
        "Bring the seedlings inside",
    ];
    let start = Arc::new(Barrier::new(titles.len()));
    let mut workers = Vec::new();
    for title in titles {
        let start = start.clone();
        let path = path.clone();
        workers.push(tokio::spawn(async move {
            start.wait().await;
            let options = OpenOptions {
                lock_timeout: Duration::from_secs(2),
                ..OpenOptions::default()
            };
            let mut app = App::open_with_options(path, NOW, options).await.unwrap();
            let request = serde_json::from_value(json!({"op":"add_task","title":title})).unwrap();
            let Response::Write {
                created: Some(id),
                changed: true,
                ..
            } = app.handle(request, NOW).await.unwrap()
            else {
                panic!("expected a created task");
            };
            (id, app.local_state().board_id)
        }));
    }

    let app = App::open(&path, NOW).await.unwrap();
    let board = app.local_state().board_id;
    let mut ids = BTreeSet::new();
    for worker in workers {
        let (id, observed_board) = worker.await.unwrap();
        assert_eq!(observed_board, board);
        assert!(ids.insert(id));
    }
    let store = pepino_store::Store::open(path.join("pepino.sqlite"))
        .await
        .unwrap();
    let tasks = store.tasks(board).await.unwrap();
    assert_eq!(
        tasks
            .iter()
            .map(|task| task.title.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(titles),
    );
    assert!(
        tasks
            .windows(2)
            .all(|pair| pair[0].created < pair[1].created)
    );
}

#[tokio::test]
async fn lock_wait_expires_with_a_retryable_busy_error() {
    use std::fs::File;
    use std::time::Duration;

    use pepino_app::OpenOptions;
    use tokio::time::Instant;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    App::open(path, NOW).await.unwrap();
    let guard = File::options()
        .read(true)
        .write(true)
        .open(path.join("local.json"))
        .unwrap();
    guard.try_lock().unwrap();
    let timeout = Duration::from_millis(20);
    let options = OpenOptions {
        lock_timeout: timeout,
        ..OpenOptions::default()
    };

    let start = Instant::now();
    let error = App::open_with_options(path, NOW, options)
        .await
        .err()
        .unwrap();
    assert!(start.elapsed() >= timeout);
    assert_eq!(error, AppError::Busy);
    assert!(error.retryable());
    assert_eq!(
        error.to_string(),
        "another pepino process holds the data directory"
    );

    drop(guard);
    App::open(path, NOW).await.unwrap();
}

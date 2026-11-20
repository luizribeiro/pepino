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

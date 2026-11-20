use pepino_app::{App, AppError, ReadRequest, Request};

#[tokio::test]
async fn get_distinguishes_invalid_and_missing_references_on_a_new_board() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open(directory.path(), 1_795_144_800_000)
        .await
        .unwrap();
    for task in ["97ad", "non-hex"] {
        let error = app
            .handle(
                Request::Read(ReadRequest::GetTask { task: task.into() }),
                1_795_144_800_000,
            )
            .await
            .unwrap_err();
        if task == "97ad" {
            assert_eq!(
                error,
                AppError::NotFound {
                    field: "task".into(),
                    task: task.into()
                }
            );
        } else {
            assert!(matches!(error, AppError::InvalidInput { field, .. } if field == "task"));
        }
    }
    assert!(directory.path().join("pepino.sqlite").is_file());
}

#[cfg(unix)]
#[tokio::test]
async fn creates_private_data_directory_and_database_files() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("greenhouse");
    let _app = App::open(&path, 1_795_144_800_000).await.unwrap();
    let mode = |file: &std::path::Path| fs::metadata(file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o700);
    for file in [
        "local.json",
        "local.lock",
        "pepino.sqlite",
        "pepino.sqlite-wal",
        "pepino.sqlite-shm",
    ] {
        assert_eq!(mode(&path.join(file)), 0o600, "{file}");
    }
}

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
            assert_eq!(error, AppError::NotFound { task: task.into() });
        } else {
            assert!(matches!(error, AppError::InvalidInput { field, .. } if field == "task"));
        }
    }
    assert!(directory.path().join("pepino.sqlite").is_file());
}

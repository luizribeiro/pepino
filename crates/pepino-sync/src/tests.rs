mod directory;
mod stub;

#[tokio::test]
async fn opens_only_the_local_board_and_joins_shutdown() {
    use crate::{Error, SyncEngine, Transport};
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let board = pepino_core::BoardId::from_bytes([16; 32]);
    let device = pepino_core::DeviceId::from_bytes([11; 32]);
    directory::prepare(dir.path(), board, device).await;
    let other = Arc::new(stub::Stub::new(pepino_core::DeviceId::from_bytes([12; 32])));
    assert!(matches!(
        SyncEngine::start(Default::default(), other, dir.path()).await,
        Err(Error::DeviceMismatch { .. })
    ));
    let node = Arc::new(stub::Stub::new(device));
    let engine = SyncEngine::start(Default::default(), node.clone(), dir.path())
        .await
        .unwrap();
    assert_eq!(
        engine.status().boards.keys().copied().collect::<Vec<_>>(),
        [board]
    );
    assert!(node.status().boards.contains_key(&board));
    engine.shutdown().await.unwrap();
    assert!(node.status().boards.is_empty());
}

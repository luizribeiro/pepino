mod directory;
mod ingest;
mod network;
mod property;
mod publish;
mod simulation;
mod stub;
mod wire;

#[tokio::test]
async fn opens_all_local_boards_and_joins_shutdown() {
    use crate::{Error, SyncEngine, Transport};
    use std::sync::Arc;

    let dir = tempfile::tempdir().unwrap();
    let board = pepino_core::BoardId::from_bytes([16; 32]);
    let device = pepino_core::DeviceId::from_bytes([11; 32]);
    let app = directory::prepare(dir.path(), board, device).await;
    let second = pepino_core::BoardId::from_bytes([17; 32]);
    let path = dir.path().join("local.json");
    let mut local = serde_json::to_value(app.local_state()).unwrap();
    drop(app);
    local["boards"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": second, "name": "Workshop"
        }));
    std::fs::write(path, local.to_string()).unwrap();
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
        [board, second]
    );
    assert!(node.status().boards.contains_key(&board));
    use futures_util::StreamExt;
    let mut changes = engine.status_changes().boxed();
    let signal = async {
        while let Some(status) = changes.next().await {
            if status.last_node_event == Some(pepino_node::NodeEvent::PeerSeen { peer: device }) {
                return;
            }
        }
        panic!("node signal stream ended");
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), signal)
        .await
        .unwrap();
    engine.shutdown().await.unwrap();
    assert!(node.status().boards.is_empty());
}

#[tokio::test]
async fn rejected_head_retries_before_its_receipt_is_recorded() {
    use self::{directory, stub::Stub};
    use crate::{Status, Transport, publication::publish};
    use pepino_core::BoardId;
    use pepino_core::DeviceId;
    use serde_json::json;
    use tokio::sync::watch;

    let dir = tempfile::tempdir().unwrap();
    let id = BoardId::from_bytes([36; 32]);
    let author = DeviceId::from_bytes([37; 32]);
    let mut app = directory::prepare(dir.path(), id, author).await;
    directory::write(
        &mut app,
        json!({"op":"add_task","title":"Install a sump pump"}),
    )
    .await;
    let event = app
        .store()
        .outbox(id, author, 1.try_into().unwrap())
        .await
        .unwrap()
        .events
        .remove(0);
    let node = Stub::new(author);
    node.reject(true);
    let board = node.open_board(id).await.unwrap();
    let (status, mut updates) = watch::channel(Status::default());
    let expected = event.bytes.clone();
    let release = async {
        updates
            .wait_for(|state| {
                state
                    .boards
                    .get(&id)
                    .is_some_and(|state| state.last_error.is_some())
            })
            .await
            .unwrap();
        assert!(node.published().is_empty());
        assert_eq!(
            app.store()
                .outbox(id, author, 1.try_into().unwrap())
                .await
                .unwrap()
                .pending,
            1
        );
        node.reject(false);
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(
            publish(&app, id, &board, node.instance_id(), &status, event),
            release
        );
    })
    .await
    .unwrap();
    assert_eq!(node.published(), [expected]);
    assert_eq!(
        app.store()
            .outbox(id, author, 1.try_into().unwrap())
            .await
            .unwrap()
            .pending,
        0
    );
}

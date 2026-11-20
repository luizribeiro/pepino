use std::{sync::Arc, time::Duration};

use pepino_core::{BoardId, DeviceId};
use serde_json::json;

use super::{directory, stub::Stub};
use crate::SyncEngine;

#[tokio::test]
async fn malformed_payloads_are_acknowledged_and_ingest_continues() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([15; 32]);
    let local = DeviceId::from_bytes([16; 32]);
    let peer = DeviceId::from_bytes([17; 32]);
    let app = directory::prepare(&dir.path().join("left"), board, local).await;
    let mut sender = directory::prepare(&dir.path().join("right"), board, peer).await;
    directory::write(
        &mut sender,
        json!({"op":"add_task","title":"Install rain barrel"}),
    )
    .await;
    let node = Arc::new(Stub::new(local));
    let engine = SyncEngine::start(Default::default(), node.clone(), dir.path().join("left"))
        .await
        .unwrap();
    let bytes = sender
        .store()
        .outbox(board, peer, 1.try_into().unwrap())
        .await
        .unwrap()
        .events
        .remove(0)
        .bytes;
    for mut receipt in [
        node.deliver(board, peer, vec![0xff]),
        node.deliver(board, peer, bytes),
    ] {
        tokio::time::timeout(Duration::from_secs(5), receipt.wait_for(|done| *done))
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(
        app.store().tasks(board).await.unwrap()[0].title,
        "Install rain barrel"
    );
    assert_eq!(app.store().statistics(board).await.unwrap().total, 1);
    assert!(
        engine.status().boards[&board]
            .last_error
            .as_ref()
            .unwrap()
            .contains("invalid event CBOR")
    );
    engine.shutdown().await.unwrap();
}

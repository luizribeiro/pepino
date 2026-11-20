use std::{sync::Arc, time::Duration};

use futures_util::StreamExt;
use pepino_core::{BoardId, DeviceId};
use serde_json::json;

use super::{directory, stub::Stub};
use crate::{Status, SyncEngine};

pub(super) async fn until(engine: &SyncEngine<Stub>, condition: impl Fn(&Status) -> bool) {
    let mut updates = engine.status_changes().boxed();
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(status) = updates.next().await {
            if condition(&status) {
                return;
            }
        }
        panic!("engine stopped before publishing the expected status");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn publishes_existing_local_writes_in_store_order() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([36; 32]);
    let device = DeviceId::from_bytes([37; 32]);
    let mut app = directory::prepare(dir.path(), board, device).await;
    for title in ["Buy a sump pump", "Install the sump pump"] {
        directory::write(&mut app, json!({"op":"add_task","title":title})).await;
    }
    let events = app
        .store()
        .outbox(board, device, 2.try_into().unwrap())
        .await
        .unwrap()
        .events;
    let node = Arc::new(Stub::new(device));
    let engine = SyncEngine::start(Default::default(), node.clone(), dir.path())
        .await
        .unwrap();
    until(&engine, Status::is_idle).await;
    assert_eq!(
        engine.status().boards[&board].last_published,
        Some(events[1].id)
    );
    assert_eq!(
        node.published(),
        events
            .into_iter()
            .map(|item| item.bytes)
            .collect::<Vec<_>>()
    );
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn idle_wait_scans_writes_that_arrived_after_an_empty_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([46; 32]);
    let device = DeviceId::from_bytes([47; 32]);
    let mut app = directory::prepare(dir.path(), board, device).await;
    let settings = crate::Settings {
        poll_interval: Duration::from_secs(600),
        ..Default::default()
    };
    let node = Arc::new(Stub::new(device));
    let engine = SyncEngine::start(settings, node.clone(), dir.path())
        .await
        .unwrap();
    engine.wait_idle(Duration::from_secs(5)).await.unwrap();
    directory::write(
        &mut app,
        json!({"op":"add_task","title":"Repaint garden bench"}),
    )
    .await;
    engine.wait_idle(Duration::from_secs(5)).await.unwrap();
    assert_eq!(node.published().len(), 1);
    assert!(engine.status().is_idle());
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_periodic_scan_recovers_a_missed_nudge() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([48; 32]);
    let device = DeviceId::from_bytes([49; 32]);
    let mut app = directory::prepare(dir.path(), board, device).await;
    let settings = crate::Settings {
        poll_interval: Duration::from_millis(25),
        ..Default::default()
    };
    let node = Arc::new(Stub::new(device));
    let engine = SyncEngine::start(settings, node.clone(), dir.path())
        .await
        .unwrap();
    engine.wait_idle(Duration::from_secs(5)).await.unwrap();
    directory::write(
        &mut app,
        json!({"op":"add_task","title":"Rehang the potting-shed door"}),
    )
    .await;
    until(&engine, |status| {
        status.boards[&board].last_published.is_some()
    })
    .await;
    assert_eq!(node.published().len(), 1);
    engine.shutdown().await.unwrap();
}

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

#[tokio::test]
async fn a_crash_after_confirmation_resends_once_and_the_receiver_deduplicates() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([50; 32]);
    let local = DeviceId::from_bytes([51; 32]);
    let peer = DeviceId::from_bytes([52; 32]);
    let mut app = directory::prepare(dir.path(), board, local).await;
    directory::write(
        &mut app,
        json!({"op":"add_task","title":"Replace the gate hinges"}),
    )
    .await;
    let node = Arc::new(Stub::new(local));
    node.reject(true);
    let settings = crate::Settings {
        lock_timeout: Duration::ZERO,
        ..Default::default()
    };
    let engine = SyncEngine::start(settings, node.clone(), dir.path())
        .await
        .unwrap();
    until(&engine, |state| state.boards[&board].last_error.is_some()).await;
    let lock = std::fs::File::options()
        .write(true)
        .open(dir.path().join("local.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    node.reject(false);
    until(&engine, |state| {
        state.boards[&board]
            .last_error
            .as_deref()
            .is_some_and(|error| error.starts_with("record publication"))
    })
    .await;
    assert_eq!(node.published().len(), 1);
    assert_eq!(
        app.store()
            .outbox(board, local, 1.try_into().unwrap())
            .await
            .unwrap()
            .pending,
        1
    );
    drop(engine);
    drop(lock);

    let restarted = Arc::new(node.restarted());
    let engine = SyncEngine::start(Default::default(), restarted.clone(), dir.path())
        .await
        .unwrap();
    engine.wait_idle(Duration::from_secs(5)).await.unwrap();
    engine.sync_once().await.unwrap();
    let sent = restarted.published();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0], sent[1]);
    engine.shutdown().await.unwrap();

    let peer_path = dir.path().join("peer");
    let receiver = directory::prepare(&peer_path, board, peer).await;
    let node = Arc::new(Stub::new(peer));
    let engine = SyncEngine::start(Default::default(), node.clone(), &peer_path)
        .await
        .unwrap();
    for bytes in sent {
        node.deliver(board, local, bytes);
    }
    engine.wait_idle(Duration::from_secs(5)).await.unwrap();
    assert_eq!(receiver.store().statistics(board).await.unwrap().total, 1);
    assert_eq!(
        receiver.store().board_with_comments(board).await.unwrap(),
        app.store().board_with_comments(board).await.unwrap()
    );
    assert!(node.published().is_empty());
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn app_writes_continue_while_publish_is_waiting_on_the_transport() {
    use pepino_app::{App, OpenOptions};

    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([53; 32]);
    let local = DeviceId::from_bytes([54; 32]);
    let mut app = directory::prepare(dir.path(), board, local).await;
    directory::write(
        &mut app,
        json!({"op":"add_task","title":"Fix the shed lock"}),
    )
    .await;
    let node = Arc::new(Stub::new(local));
    let mut started = node.pause_publish();
    let engine = SyncEngine::start(Default::default(), node, dir.path())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), started.wait_for(|started| *started))
        .await
        .unwrap()
        .unwrap();

    let options = OpenOptions {
        lock_timeout: Duration::ZERO,
        ..Default::default()
    };
    let mut cli = App::open_with_options(dir.path(), directory::NOW, options)
        .await
        .unwrap();
    directory::write(
        &mut cli,
        json!({"op":"add_task","title":"Replace the shed roof"}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), engine.shutdown())
        .await
        .expect("shutdown must cancel the blocked publish")
        .unwrap();
    assert_eq!(
        cli.store()
            .outbox(board, local, 1.try_into().unwrap())
            .await
            .unwrap()
            .pending,
        2
    );
}

#[tokio::test]
async fn publishes_a_smaller_local_event_written_after_the_first_scan() {
    use pepino_core::{Envelope, Event, Hlc, InitialFields};

    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([55; 32]);
    let device = DeviceId::from_bytes([56; 32]);
    let mut app = directory::prepare(dir.path(), board, device).await;
    directory::write(
        &mut app,
        json!({"op":"add_task","title":"Label the garden plots"}),
    )
    .await;
    let node = Arc::new(Stub::new(device));
    let engine = SyncEngine::start(Default::default(), node.clone(), dir.path())
        .await
        .unwrap();
    let limit = Duration::from_secs(5);
    engine.wait_idle(limit).await.unwrap();
    let first = node.published();
    let early = Envelope::new(
        device,
        Hlc {
            wall_ms: directory::NOW - 1,
            counter: 0,
        },
        Event::TaskCreated {
            title: "Mark water lines".into(),
            fields: InitialFields::default(),
        },
    )
    .unwrap();
    let later = Envelope::decode(&first[0]).unwrap();
    assert!((Some(early.hlc()), early.id()) < (later.hlc(), later.id()));
    let bytes = early.encode().unwrap();
    app.store().append(board, &bytes, None).await.unwrap();
    engine.wait_idle(limit).await.unwrap();
    assert_eq!(node.published(), [first[0].clone(), bytes]);
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn retries_a_failed_outbox_read_without_a_nudge_or_periodic_scan() {
    use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([57; 32]);
    let device = DeviceId::from_bytes([58; 32]);
    let mut app = directory::prepare(dir.path(), board, device).await;
    let node = Arc::new(Stub::new(device));
    let settings = crate::Settings {
        poll_interval: Duration::from_secs(600),
        ..Default::default()
    };
    let engine = SyncEngine::start(settings, node.clone(), dir.path())
        .await
        .unwrap();
    engine.wait_idle(Duration::from_secs(5)).await.unwrap();

    let options = SqliteConnectOptions::new().filename(dir.path().join("pepino.sqlite"));
    let mut db = SqliteConnection::connect_with(&options).await.unwrap();
    sqlx::query("ALTER TABLE publication RENAME TO hidden_publication")
        .execute(&mut db)
        .await
        .unwrap();
    directory::write(
        &mut app,
        json!({"op":"add_task","title":"Order replacement irrigation valves"}),
    )
    .await;
    let started = tokio::time::Instant::now();
    engine.nudge();
    until(&engine, |status| {
        status.boards[&board].outbox_pending.is_none()
            && status.boards[&board]
                .last_error
                .as_deref()
                .is_some_and(|error| error.starts_with("read outbox"))
    })
    .await;
    assert!(!engine.status().is_idle());
    assert!(node.published().is_empty());
    sqlx::query("ALTER TABLE hidden_publication RENAME TO publication")
        .execute(&mut db)
        .await
        .unwrap();
    until(&engine, Status::is_idle).await;
    assert!(started.elapsed() >= Duration::from_millis(25));
    assert_eq!(node.published().len(), 1);
    engine.shutdown().await.unwrap();
}

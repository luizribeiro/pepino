use std::{sync::Arc, time::Duration};

use pepino_core::{BoardId, DeviceId};
use serde_json::json;

use super::{directory, stub::Stub};
use crate::SyncEngine;

#[tokio::test]
async fn malformed_payloads_are_retained_before_acknowledging_and_ingest_continues() {
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
    let malformed = vec![0xff];
    for (payload, total) in [(malformed.clone(), 1), (bytes, 2)] {
        let mut receipt = node.deliver(board, peer, payload);
        tokio::time::timeout(Duration::from_secs(5), receipt.wait_for(|done| *done))
            .await
            .unwrap()
            .unwrap();
        let stats = app.store().statistics(board).await.unwrap();
        assert_eq!((stats.total, stats.undecodable), (total, 1));
        assert_eq!(
            app.store().undecodable(board).await.unwrap(),
            [(peer, malformed.clone())]
        );
    }
    assert_eq!(
        app.store().tasks(board).await.unwrap()[0].title,
        "Install rain barrel"
    );
    assert!(
        engine.status().boards[&board]
            .last_error
            .as_ref()
            .unwrap()
            .contains("invalid event CBOR")
    );
    let snapshot = engine.status();
    assert_eq!(snapshot.boards[&board].received, 1);
    assert!(snapshot.boards[&board].last_received_at.is_some());
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn waits_for_the_local_lock_before_acknowledging() {
    use futures_util::StreamExt;

    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([21; 32]);
    let device = DeviceId::from_bytes([22; 32]);
    let app = directory::prepare(dir.path(), board, device).await;
    let node = Arc::new(Stub::new(device));
    let engine = SyncEngine::start(
        crate::Settings {
            lock_timeout: Duration::ZERO,
            ..Default::default()
        },
        node.clone(),
        dir.path(),
    )
    .await
    .unwrap();
    let mut updates = engine.status_changes().boxed();
    let lock = std::fs::File::options()
        .write(true)
        .open(dir.path().join("local.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let mut receipt = node.deliver(board, device, vec![0x82, 2, 0xf6]);
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(status) = updates.next().await {
            if status.boards[&board]
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("holds the data directory"))
            {
                assert!(status.boards[&board].ingest_pending);
                return;
            }
        }
        panic!("no lock failure observed");
    })
    .await
    .unwrap();
    assert!(!*receipt.borrow());
    assert_eq!(app.store().statistics(board).await.unwrap().total, 0);
    drop(lock);
    tokio::time::timeout(Duration::from_secs(5), receipt.wait_for(|done| *done))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        app.store().statistics(board).await.unwrap().unknown_version,
        1
    );
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_crash_after_commit_redelivers_as_a_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([44; 32]);
    let local = DeviceId::from_bytes([45; 32]);
    let peer = DeviceId::from_bytes([46; 32]);
    let app = directory::prepare(dir.path(), board, local).await;
    let mut remote = directory::prepare(&dir.path().join("peer"), board, peer).await;
    directory::write(
        &mut remote,
        json!({"op":"add_task","title":"Repair the awning"}),
    )
    .await;
    let bytes = remote
        .store()
        .outbox(board, peer, 1.try_into().unwrap())
        .await
        .unwrap()
        .events
        .remove(0)
        .bytes;
    let node = Arc::new(Stub::new(local));
    let engine = SyncEngine::start(Default::default(), node.clone(), dir.path())
        .await
        .unwrap();
    let (mut attempting, receipt) = node.pause_before_ack(board, peer, bytes.clone());
    tokio::time::timeout(
        Duration::from_secs(5),
        attempting.wait_for(|started| *started),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!*receipt.borrow());
    let before = app.store().board_with_comments(board).await.unwrap();
    assert_eq!(before.tasks()[0].title, "Repair the awning");
    assert!(matches!(
        engine.wait_idle(Duration::from_millis(20)).await,
        Err(crate::Error::Timeout)
    ));
    drop(engine);

    let restarted = Arc::new(Stub::new(local));
    let engine = SyncEngine::start(Default::default(), restarted.clone(), dir.path())
        .await
        .unwrap();
    let mut redelivered = restarted.deliver(board, peer, bytes);
    tokio::time::timeout(Duration::from_secs(5), redelivered.wait_for(|done| *done))
        .await
        .unwrap()
        .unwrap();
    engine.wait_idle(Duration::from_secs(5)).await.unwrap();
    assert_eq!(
        app.store().board_with_comments(board).await.unwrap(),
        before
    );
    assert_eq!(app.store().statistics(board).await.unwrap().total, 1);
    assert_eq!(engine.status().boards[&board].last_ingested, None);
    assert!(restarted.published().is_empty());
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn waits_for_a_connected_peers_first_pull_and_ingests_its_events() {
    use pepino_core::{Envelope, Event, Hlc, InitialFields};
    use std::task::Poll;

    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([61; 32]);
    let local = DeviceId::from_bytes([62; 32]);
    let peer = DeviceId::from_bytes([63; 32]);
    let app = directory::prepare(dir.path(), board, local).await;
    let node = Arc::new(Stub::new(local));
    let engine = SyncEngine::start(Default::default(), node.clone(), dir.path())
        .await
        .unwrap();
    let limit = Duration::from_secs(5);
    engine.wait_idle(limit).await.unwrap();
    assert!(engine.status().is_idle());
    assert!(engine.status().node.connected_peers.is_empty());
    node.connect(peer);

    {
        let idle = engine.wait_idle(limit);
        tokio::pin!(idle);
        assert_eq!(futures_util::poll!(&mut idle).map(|_| ()), Poll::Pending);
        super::publish::until(&engine, |status| {
            status.boards[&board].scan_generation == Some(2)
        })
        .await;
        assert!(!engine.status().is_idle());
        assert_eq!(futures_util::poll!(&mut idle).map(|_| ()), Poll::Pending);
        let event = Envelope::new(
            peer,
            Hlc {
                wall_ms: 100,
                counter: 0,
            },
            Event::TaskCreated {
                title: "Count the seed trays".into(),
                fields: InitialFields::default(),
            },
        )
        .unwrap();
        let ack = node.deliver(board, peer, event.encode().unwrap());
        node.caught_up(board, peer);
        idle.await.unwrap();
        assert!(*ack.borrow());
    }
    assert_eq!(
        app.store().tasks(board).await.unwrap()[0].title,
        "Count the seed trays"
    );
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn stream_errors_back_off_and_keep_the_board_non_idle_until_repoll() {
    use futures_util::{StreamExt, stream};
    use std::time::Duration;
    use tokio::{sync::watch, time::Instant};

    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([65; 32]);
    let app = directory::prepare(dir.path(), board, DeviceId::from_bytes([66; 32])).await;
    tokio::time::pause();
    let mut idle = crate::Status::default();
    idle.boards.entry(board).or_default().outbox_pending = Some(0);
    idle.node.boards.insert(board, Default::default());
    let (status, _) = watch::channel(idle);
    let (polled, mut poll_times) = tokio::sync::mpsc::unbounded_channel();
    let attempt = |error: &str| {
        Err::<crate::Delivery<super::stub::Receipt>, _>(pepino_node::Error::Node(error.into()))
    };
    let stream = stream::iter([attempt("interrupted"), attempt("still interrupted")])
        .map(move |item| {
            polled.send(Instant::now()).unwrap();
            item
        })
        .chain(stream::pending())
        .boxed();
    let (started, mut states) = watch::channel(false);
    let observe = async {
        let first = poll_times.recv().await.unwrap();
        started.send_replace(true);
        let second = poll_times.recv().await.unwrap();
        assert!(second - first >= Duration::from_millis(25));
    };
    let mut updates = status.subscribe();
    let seq = async {
        states.wait_for(|started| *started).await.unwrap();
        let retrying = updates
            .wait_for(|status| status.boards[&board].stream_retrying)
            .await
            .unwrap();
        assert!(!retrying.is_idle());
        drop(retrying);
        let idle = updates.wait_for(crate::Status::is_idle).await.unwrap();
        assert!(
            idle.boards[&board]
                .last_error
                .as_deref()
                .unwrap()
                .contains("still interrupted")
        );
    };
    tokio::select! {
        result = crate::inbox::run(&app, board, stream, &status) => panic!("{result:?}"),
        _ = async { tokio::join!(observe, seq); } => (),
    }
}

#[tokio::test]
async fn retries_ack_without_appending_again() {
    use super::stub::Receipt;
    use pepino_core::EventId;

    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([74; 32]);
    let device = DeviceId::from_bytes([75; 32]);
    let app = directory::prepare(dir.path(), board, device).await;
    let node = Arc::new(Stub::new(device));
    let engine = SyncEngine::start(Default::default(), node.clone(), dir.path())
        .await
        .unwrap();
    let ack = Receipt::new();
    node.deliver_operation(
        board,
        EventId::from_bytes([76; 32]),
        device,
        vec![0x82, 2, 0xf6],
        ack.clone().with_failures(2),
    );
    super::publish::until(&engine, |s| {
        s.boards[&board]
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("test acknowledgement failure"))
    })
    .await;
    assert_eq!(
        app.store().statistics(board).await.unwrap().unknown_version,
        1
    );
    engine.wait_idle(Duration::from_secs(5)).await.unwrap();
    assert!(ack.is_acked());
    assert_eq!(app.store().statistics(board).await.unwrap().total, 1);
    engine.shutdown().await.unwrap();
}

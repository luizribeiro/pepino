use std::{io::Write, path::Path, time::Instant};

use futures_util::StreamExt;
use pepino_node::{MdnsMode, NodeEvent, NodeEvents};

use super::support::{WAIT, fixture_dir, network, next, settings, start};

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn restart_skips_acknowledged_history() {
    let dir = fixture_dir();
    check_restart(dir.path(), 48, 8).await;
}

#[tokio::test]
#[ignore = "builds a 3,000-event encrypted history"]
async fn restart_skips_acknowledged_history_at_scale() {
    let dir = tempfile::tempdir().unwrap();
    check_restart(dir.path(), 3_000, 50).await;
}

async fn check_restart(dir: &Path, history: usize, tail: usize) {
    let mut config = settings(&network(dir), dir.join("alice"));
    config.mdns = MdnsMode::Disabled;
    let node = start(&config).await;
    let mut board = node.create_board().await.unwrap();
    let board_id = board.id();
    let mut events = board.events().unwrap();
    let mut expected = Vec::with_capacity(tail);
    for index in 0..history {
        let bytes = format!("Review release checklist item {index}").into_bytes();
        let id = board.publish(bytes.clone()).await.unwrap();
        let event = next(&mut events).await;
        assert_eq!((event.id, &event.bytes), (id, &bytes));
        if index < history - tail {
            event.ack().await.unwrap();
        } else {
            expected.push((id, bytes));
        }
    }
    node.shutdown().await.unwrap();

    let node = start(&config).await;
    let status = node.events();
    let mut board = node.open_board(board_id).await.unwrap();
    let mut events = board.events().unwrap();
    for (id, bytes) in expected {
        let event = next(&mut events).await;
        assert_eq!((event.id, &event.bytes), (id, &bytes));
        event.ack().await.unwrap();
    }
    board.wait_until_joined(WAIT).await.unwrap();
    board.wait_until_replayed(WAIT).await.unwrap();
    let sentinel = board
        .publish(b"Close the release checklist".to_vec())
        .await
        .unwrap();
    let event = next(&mut events).await;
    assert_eq!(event.id, sentinel);
    event.ack().await.unwrap();
    board.close().await.unwrap();
    let partial = replayed_operations(status).await;
    node.shutdown().await.unwrap();

    let beginning = Instant::now();
    let node = start(&config).await;
    let status = node.events();
    let mut board = node.open_board(board_id).await.unwrap();
    board.wait_until_joined(WAIT).await.unwrap();
    board.wait_until_replayed(WAIT).await.unwrap();
    let sentinel = board
        .publish(b"Open the next release checklist".to_vec())
        .await
        .unwrap();
    let elapsed = beginning.elapsed();
    let event = next(&mut board.events().unwrap()).await;
    assert_eq!(event.id, sentinel);
    board.close().await.unwrap();
    let full = replayed_operations(status).await;
    assert_eq!(partial, full + u32::try_from(tail).unwrap());
    // The member and space control logs have no application receipts.
    assert!(full <= 3, "unexpected replay of {full} operations");
    writeln!(
        std::io::stderr().lock(),
        "fully acknowledged restart and publish: {elapsed:?} ({full} replay operations)"
    )
    .unwrap();
    node.shutdown().await.unwrap();
}

async fn replayed_operations(mut status: NodeEvents) -> u32 {
    let mut operations = 0;
    while let Some(event) = status.next().await {
        match event.unwrap() {
            NodeEvent::ReplayStarted {
                operations: count, ..
            } => operations += count,
            NodeEvent::ReplayEnded { error, .. } => assert_eq!(error, None),
            NodeEvent::BoardClosed { .. } => return operations,
            _ => (),
        }
    }
    panic!("board closure missing from status");
}

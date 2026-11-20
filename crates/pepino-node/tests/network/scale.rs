use std::{
    io::Write,
    time::{Duration, Instant},
};

use tokio::time::timeout;

use super::support::{WAIT, network, next, settings, start};

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn restart_skips_acknowledged_history() {
    const HISTORY: usize = 3_000;
    const TAIL: usize = 50;
    let dir = tempfile::tempdir().unwrap();
    let config = settings(&network(dir.path()), dir.path().join("alice"));
    let node = start(&config).await;
    let mut board = node.create_board().await.unwrap();
    let board_id = board.id();
    let mut events = board.events().unwrap();
    let mut expected = Vec::with_capacity(TAIL);
    for index in 0..HISTORY {
        let bytes = format!("Review release checklist item {index}").into_bytes();
        let id = board.publish(bytes.clone()).await.unwrap();
        let event = next(&mut events).await;
        assert_eq!((event.id, &event.bytes), (id, &bytes));
        if index < HISTORY - TAIL {
            event.ack().await.unwrap();
        } else {
            expected.push((id, bytes));
        }
    }
    node.shutdown().await.unwrap();

    let node = start(&config).await;
    let mut board = node.open_board(board_id).await.unwrap();
    assert!(matches!(
        board.publish(b"Must not queue behind replay".to_vec()).await,
        Err(pepino_node::Error::PublishNotSubmitted(reason)) if reason.contains("local replay")
    ));
    let mut events = board.events().unwrap();
    for (id, bytes) in expected {
        let event = next(&mut events).await;
        assert_eq!((event.id, &event.bytes), (id, &bytes));
        event.ack().await.unwrap();
    }
    board.wait_until_replayed(WAIT).await.unwrap();
    let sentinel = board
        .publish(b"Close the release checklist".to_vec())
        .await
        .unwrap();
    let event = next(&mut events).await;
    assert_eq!(event.id, sentinel);
    event.ack().await.unwrap();
    node.shutdown().await.unwrap();

    let beginning = Instant::now();
    let node = timeout(Duration::from_secs(2), async {
        let node = start(&config).await;
        let board = node.open_board(board_id).await.unwrap();
        board.wait_until_joined(WAIT).await.unwrap();
        board.wait_until_replayed(WAIT).await.unwrap();
        board
            .publish(b"Open the next release checklist".to_vec())
            .await
            .unwrap();
        node
    })
    .await
    .unwrap();
    let elapsed = beginning.elapsed();
    assert!(elapsed < Duration::from_secs(2));
    writeln!(
        std::io::stderr().lock(),
        "fully acknowledged restart and publish: {elapsed:?}"
    )
    .unwrap();
    node.shutdown().await.unwrap();
}

use std::{
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    time::Duration,
};

use futures_util::future::try_join_all;
use pepino_node::{BoardHandle, Error, Node, Role};
use tempfile::TempDir;
use tokio::time::timeout;

use super::support::{WAIT, enroll, network, settings, start, until};

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn two_and_three_nodes_shut_down_concurrently_after_board_close() {
    for names in [&["alice", "ben"][..], &["alice", "ben", "chloe"][..]] {
        let (_dir, nodes) = close_enrolled_board(names).await;
        try_join_all(nodes.into_iter().map(Node::shutdown))
            .await
            .unwrap();
    }
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn two_and_three_nodes_shut_down_sequentially_after_board_close() {
    for names in [&["alice", "ben"][..], &["alice", "ben", "chloe"][..]] {
        let (_dir, nodes) = close_enrolled_board(names).await;
        for node in nodes {
            node.shutdown().await.unwrap();
        }
    }
}

async fn close_enrolled_board(names: &[&str]) -> (TempDir, Vec<Node>) {
    let dir = super::support::fixture_dir();
    let network = network(dir.path());
    let mut nodes = Vec::new();
    for name in names {
        nodes.push(start(&settings(&network, dir.path().join(name))).await);
    }
    let board = nodes[0].create_board().await.unwrap();
    board
        .publish(b"Review the release checklist".to_vec())
        .await
        .unwrap();
    let mut boards = vec![board];
    for node in &nodes[1..] {
        boards.push(enroll(&boards[0], node, Role::Write).await);
    }
    try_join_all(boards.into_iter().map(BoardHandle::close))
        .await
        .unwrap();
    (dir, nodes)
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn shutdown_releases_the_udp_port_with_a_board_still_open() {
    let (_dir, node, address) = node_with_udp_port().await;
    let board = node.create_board().await.unwrap();
    node.shutdown().await.unwrap();
    assert!(matches!(
        board.wait_until_joined(WAIT).await,
        Err(Error::Closed)
    ));
    wait_for_udp_port(address).await;
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn shutdown_immediately_after_spawn_releases_the_udp_port() {
    let (_dir, node, address) = node_with_udp_port().await;
    node.shutdown().await.unwrap();
    wait_for_udp_port(address).await;
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn dropping_without_shutdown_closes_an_open_board() {
    let (dir, node, address) = node_with_udp_port().await;
    let board = node.create_board().await.unwrap();
    board
        .publish(b"Verify the rollback plan".to_vec())
        .await
        .unwrap();
    drop(node);
    let lock = std::fs::File::options()
        .write(true)
        .open(dir.path().join("alice/node.lock"))
        .unwrap();
    until(async || lock.try_lock(), Result::is_ok, WAIT)
        .await
        .unwrap();
    assert!(matches!(
        board.wait_until_joined(WAIT).await,
        Err(Error::Closed)
    ));
    wait_for_udp_port(address).await;
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn shutdown_closes_a_board_waiting_for_membership() {
    let dir = super::support::fixture_dir();
    let network = network(dir.path());
    let owner = start(&settings(&network, dir.path().join("alice"))).await;
    let board = owner.create_board().await.unwrap();
    let peer = start(&settings(&network, dir.path().join("ben"))).await;
    let joining = peer.open_board(board.id()).await.unwrap();
    let joined = joining.wait_until_joined(WAIT);
    tokio::pin!(joined);
    assert!(futures_util::poll!(&mut joined).is_pending());
    peer.shutdown().await.unwrap();
    assert!(matches!(joined.await, Err(Error::Closed)));
    owner.shutdown().await.unwrap();
}

async fn node_with_udp_port() -> (TempDir, Node, SocketAddr) {
    let dir = tempfile::tempdir().unwrap();
    let mut settings = settings(&network(dir.path()), dir.path().join("alice"));
    let address = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap();
    settings.bind_port = Some(address.port());
    let node = start(&settings).await;
    assert_eq!(
        UdpSocket::bind(address).unwrap_err().kind(),
        std::io::ErrorKind::AddrInUse
    );
    (dir, node, address)
}

async fn wait_for_udp_port(address: SocketAddr) {
    let bind = async || UdpSocket::bind(address);
    let _socket = until(bind, Result::is_ok, WAIT).await;
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn closed_board_can_be_reopened_on_a_restarted_node() {
    let dir = tempfile::tempdir().unwrap();
    let settings = settings(&network(dir.path()), dir.path().join("alice"));
    let node = start(&settings).await;
    let board = node.create_board().await.unwrap();
    let id = board.id();
    board.close().await.unwrap();
    assert!(
        matches!(node.open_board(id).await, Err(pepino_node::Error::BoardRestartRequired(board)) if board == id)
    );
    assert!(matches!(
        node.open_board_with(id, pepino_node::OpenBoardOptions { from_start: true }).await,
        Err(pepino_node::Error::BoardRestartRequired(board)) if board == id
    ));
    node.shutdown().await.unwrap();
    let node = start(&settings).await;
    let board = node.open_board(id).await.unwrap();
    board.wait_until_joined(WAIT).await.unwrap();
    board.wait_until_replayed(WAIT).await.unwrap();
    board
        .publish(b"Roll out the migration".to_vec())
        .await
        .unwrap();
    node.shutdown().await.unwrap();
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn late_status_subscriber_sees_deliveries_waiting_for_the_consumer() {
    use super::support::next;
    use futures_util::StreamExt;
    use pepino_node::NodeEvent;

    let dir = tempfile::tempdir().unwrap();
    let node = start(&settings(&network(dir.path()), dir.path().join("alice"))).await;
    let mut board = node.create_board().await.unwrap();
    let id = board.id();
    let expected = board
        .publish(b"Prepare the release handoff".to_vec())
        .await
        .unwrap();
    assert_eq!(node.status().boards[&id].pending_deliveries, 1);

    let mut late = node.events();
    let NodeEvent::Snapshot(snapshot) = late.next().await.unwrap().unwrap() else {
        panic!("expected current node state");
    };
    assert_eq!(snapshot.boards[&id].pending_deliveries, 1);
    let event = next(&mut board.events().unwrap()).await;
    assert_eq!(event.id, expected);
    timeout(WAIT, async {
        while late.next().await.unwrap().unwrap()
            != (NodeEvent::PendingDeliveries {
                board: id,
                count: 0,
            })
        {}
    })
    .await
    .unwrap();
    event.ack().await.unwrap();
    node.shutdown().await.unwrap();
    timeout(WAIT, async { while late.next().await.is_some() {} })
        .await
        .unwrap();
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn publish_reports_uncertainty_at_the_configured_deadline() {
    use pepino_node::Error;
    use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};

    let dir = tempfile::tempdir().unwrap();
    let mut config = settings(&network(dir.path()), dir.path().join("alice"));
    config.publish_confirmation_timeout = Duration::from_millis(25);
    let node = start(&config).await;
    let board = node.create_board().await.unwrap();
    board.wait_until_replayed(WAIT).await.unwrap();
    let pool =
        SqlitePool::connect_with(SqliteConnectOptions::new().filename(&config.database_path))
            .await
            .unwrap();
    let writer = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();

    let result = timeout(WAIT, board.publish(b"Check deployment status".to_vec()))
        .await
        .unwrap();
    assert!(
        matches!(result, Err(Error::PublishUnconfirmed(reason)) if reason.contains("timed out"))
    );
    writer.rollback().await.unwrap();
    pool.close().await;
    node.shutdown().await.unwrap();
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn peer_hint_with_no_relay_uses_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let node = start(&settings(&network(dir.path()), dir.path().join("titanium"))).await;
    let peer = pepino_node::DeviceCredentials::load_or_create(dir.path().join("ferrum.cbor"))
        .unwrap()
        .device_id();
    node.register_peer_hint(peer, None).await.unwrap();
    assert!(matches!(
        node.register_peer_hint(peer, Some("udp://relay.example.org"))
            .await,
        Err(pepino_node::Error::Settings(_))
    ));
    node.shutdown().await.unwrap();
}

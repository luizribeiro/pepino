use std::{
    net::{Ipv4Addr, UdpSocket},
    time::Duration,
};

use tokio::time::{sleep, timeout};

use super::support::{network, settings, start};

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn shutdown_releases_the_udp_port_with_a_board_still_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut settings = settings(&network(dir.path()), dir.path().join("alice"));
    let address = Ipv4Addr::LOCALHOST;
    let port = UdpSocket::bind((address, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    settings.bind_port = Some(port);
    let node = start(&settings).await;
    let _board = node.create_board().await.unwrap();
    assert!(UdpSocket::bind((address, port)).is_err());
    node.shutdown().await.unwrap();
    assert!(UdpSocket::bind((address, port)).is_ok());
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn dropping_without_shutdown_releases_the_udp_port() {
    let dir = tempfile::tempdir().unwrap();
    let mut settings = settings(&network(dir.path()), dir.path().join("alice"));
    let address = Ipv4Addr::LOCALHOST;
    let port = UdpSocket::bind((address, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    settings.bind_port = Some(port);
    let node = start(&settings).await;
    assert!(UdpSocket::bind((address, port)).is_err());
    drop(node);
    timeout(Duration::from_secs(2), async {
        while UdpSocket::bind((address, port)).is_err() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
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
    board
        .wait_until_joined(Duration::from_secs(1))
        .await
        .unwrap();
    board
        .publish(b"Roll out the migration".to_vec())
        .await
        .unwrap();
    node.shutdown().await.unwrap();
}

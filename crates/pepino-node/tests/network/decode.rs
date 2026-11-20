use std::net::Ipv4Addr;

use futures_util::StreamExt;
use p2panda::{AccessLevel, Hash, spaces::Member};
use pepino_core::BoardId;
use pepino_node::Error;
use tokio::time::timeout;

use super::support::{WAIT, network, next, settings, start};

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn a_decode_error_leaves_the_board_stream_open() {
    let dir = tempfile::tempdir().unwrap();
    let network = network(dir.path());
    let raw = p2panda::builder()
        .network_id(Hash::digest(network.as_bytes()).into())
        .bind_ip_v4(Ipv4Addr::LOCALHOST)
        .spawn()
        .await
        .unwrap();
    let (space, _subscription) = raw.create_space::<String>().await.unwrap();
    let node = start(&settings(&network, dir.path().join("reader"))).await;
    let mut board = node
        .open_board(BoardId::from_bytes(*space.id().as_bytes()))
        .await
        .unwrap();
    let member = Member::from_bytes(node.member_info().await.unwrap().as_bytes()).unwrap();
    let id = member.id();
    raw.register_member(member).await.unwrap();
    space.add(id, AccessLevel::Write).await.unwrap();
    board.wait_until_joined(WAIT).await.unwrap();
    let mut events = board.events().unwrap();

    space
        .publish("legacy unframed message".into())
        .await
        .unwrap()
        .await
        .unwrap();
    assert!(matches!(
        timeout(WAIT, events.next()).await.unwrap(),
        Some(Err(Error::Decode { .. }))
    ));

    let id = board
        .publish(b"Retest the migration".to_vec())
        .await
        .unwrap();
    let event = next(&mut events).await;
    assert_eq!(event.id, id);
    assert_eq!(event.bytes, b"Retest the migration");
    event.ack().await.unwrap();
    node.shutdown().await.unwrap();
}

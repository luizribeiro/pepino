use std::{
    io::Write,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    time::Duration,
};

use futures_util::StreamExt;
use pepino_core::{DeviceId, EventId};
use pepino_node::{
    BoardEvent, BoardEvents, BoardHandle, DeviceCredentials, Node, NodeSettings, Role,
};
use tokio::time::timeout;

pub const WAIT: Duration = Duration::from_secs(8);

pub fn enabled() -> bool {
    let enabled = std::env::var("PEPINO_NETWORK_TESTS").as_deref() == Ok("1");
    if !enabled {
        writeln!(
            std::io::stderr().lock(),
            "skipping UDP test; run with PEPINO_NETWORK_TESTS=1"
        )
        .unwrap();
    }
    enabled
}

pub fn network(dir: &Path) -> String {
    format!(
        "pepino-network-{}",
        dir.file_name().unwrap().to_string_lossy()
    )
}

pub fn settings(network: &str, dir: PathBuf) -> NodeSettings {
    std::fs::create_dir_all(&dir).unwrap();
    let keys = DeviceCredentials::load_or_create(dir.join("device.cbor")).unwrap();
    let mut settings = NodeSettings::new(keys, dir.join("node.sqlite"));
    settings.network_name = network.into();
    settings.bind_ip_v4 = Some(Ipv4Addr::LOCALHOST);
    settings
}

pub async fn start(settings: &NodeSettings) -> Node {
    timeout(WAIT, Node::spawn(settings.clone()))
        .await
        .unwrap()
        .unwrap()
}

pub async fn enroll(owner: &BoardHandle, device: &Node, role: Role) -> BoardHandle {
    let board = device.open_board(owner.id()).await.unwrap();
    owner
        .add_member(&device.member_info().await.unwrap(), role)
        .await
        .unwrap();
    board.wait_until_joined(WAIT).await.unwrap();
    board
}

pub async fn next(events: &mut BoardEvents) -> BoardEvent {
    timeout(WAIT, events.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

pub async fn expect(events: &mut BoardEvents, expected: &[(EventId, DeviceId, &[u8])]) {
    let mut remaining = expected.to_vec();
    while !remaining.is_empty() {
        let event = next(events).await;
        let (id, author, bytes) = *expected.iter().find(|(id, _, _)| *id == event.id).unwrap();
        assert_eq!(
            (event.id, event.author, event.bytes.as_slice()),
            (id, author, bytes)
        );
        if let Some(index) = remaining.iter().position(|(id, _, _)| *id == event.id) {
            assert!(
                !remaining[..index]
                    .iter()
                    .any(|(_, earlier, _)| *earlier == author)
            );
            remaining.remove(index);
        }
        event.ack().await.unwrap();
    }
}

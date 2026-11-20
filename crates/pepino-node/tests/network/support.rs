use std::{
    io::Write,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    time::Duration,
};

use futures_util::StreamExt;
use pepino_node::{BoardEvent, BoardEvents, DeviceCredentials, Node, NodeSettings};
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

pub async fn next(events: &mut BoardEvents) -> BoardEvent {
    timeout(WAIT, events.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

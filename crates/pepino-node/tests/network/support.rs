use std::{
    fmt::Debug,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    time::Duration,
};

use futures_util::StreamExt;
use pepino_core::{DeviceId, EventId};
use pepino_node::{
    BoardEvent, BoardEvents, BoardHandle, DeviceCredentials, Node, NodeSettings, Role,
};
use tokio::time::{Instant, sleep, timeout};

pub const WAIT: Duration = Duration::from_secs(60);

pub async fn until<T: Debug>(
    mut observe: impl AsyncFnMut() -> T,
    condition: impl Fn(&T) -> bool,
    timeout: Duration,
) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        let observed = observe().await;
        if condition(&observed) {
            return observed;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "condition timed out after {timeout:?}. Last observation: {observed:?}"
        );
        sleep(remaining.min(Duration::from_millis(25))).await;
    }
}

pub fn fixture_dir() -> tempfile::TempDir {
    #[cfg(target_os = "linux")]
    if std::env::temp_dir() == Path::new("/tmp")
        && let Ok(dir) = tempfile::tempdir_in("/dev/shm")
    {
        return dir;
    }
    tempfile::tempdir().unwrap()
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
        .expect("node startup timed out")
        .unwrap()
}

pub async fn enroll(owner: &BoardHandle, device: &Node, role: Role) -> BoardHandle {
    let board = device.open_board(owner.id()).await.unwrap();
    owner
        .add_member(&device.member_info().await.unwrap(), role)
        .await
        .unwrap();
    board.wait_until_joined(WAIT).await.unwrap_or_else(|error| {
        panic!(
            "{} joining board {} failed with {error}. Last status was {:?}",
            device.device_id(),
            owner.id(),
            device.status()
        )
    });
    board
}

pub async fn next(events: &mut BoardEvents) -> BoardEvent {
    timeout(WAIT, events.next())
        .await
        .expect("board event timed out")
        .expect("board event stream ended")
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

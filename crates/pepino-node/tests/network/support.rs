use std::{
    fmt::Debug,
    net::{Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
    time::Duration,
};

use futures_util::StreamExt;
use p2panda_net::{addrs::NodeInfo, iroh_endpoint::EndpointAddr};
use p2panda_store::{Transaction, address_book::AddressBookStore, sqlite::SqliteStore};
use pepino_core::{BoardId, DeviceId, EventId};
use pepino_node::{
    BoardEvent, BoardEvents, BoardHandle, DeviceCredentials, MdnsMode, Node, NodeSettings, Role,
};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
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

pub fn local_settings(network: &str, dir: PathBuf) -> NodeSettings {
    let mut settings = settings(network, dir);
    settings.mdns = MdnsMode::Disabled;
    settings.bind_ip_v6 = Some(Ipv6Addr::LOCALHOST);
    settings
}

async fn address_book(settings: &NodeSettings, create: bool) -> SqlitePool {
    let options = SqliteConnectOptions::new()
        .filename(&settings.database_path)
        .create_if_missing(create)
        .journal_mode(SqliteJournalMode::Wal);
    SqlitePool::connect_with(options).await.unwrap()
}

pub async fn owner_peer_info(settings: &NodeSettings, owner: &Node) -> NodeInfo {
    let pool = address_book(settings, false).await;
    let store = SqliteStore::from_pool(pool.clone());
    let id = p2panda::VerifyingKey::from_bytes(owner.device_id().as_bytes()).unwrap();
    let endpoint = until(
        async || {
            AddressBookStore::<_, NodeInfo>::node_info(&store, &id)
                .await
                .unwrap()
                .and_then(|info| EndpointAddr::try_from(info).ok())
        },
        |endpoint| {
            endpoint
                .as_ref()
                .is_some_and(|value| value.ip_addrs().any(|addr| addr.ip().is_loopback()))
        },
        WAIT,
    )
    .await
    .unwrap();
    pool.close().await;
    let addrs = endpoint
        .ip_addrs()
        .copied()
        .filter(|addr| addr.ip().is_loopback());
    NodeInfo::from(addrs.fold(EndpointAddr::new(endpoint.id), EndpointAddr::with_ip_addr))
}

pub async fn start_peer(settings: &NodeSettings, owner: &NodeInfo, board: BoardId) -> Node {
    let pool = address_book(settings, true).await;
    p2panda_store::sqlite::run_pending_migrations(&pool)
        .await
        .unwrap();
    let store = SqliteStore::from_pool(pool.clone());
    let transaction = store.begin().await.unwrap();
    store.insert_node_info_tx(owner.clone()).await.unwrap();
    let topics = [p2panda::Topic::from(*board.as_bytes())].into();
    AddressBookStore::<_, NodeInfo>::set_topics_tx(&store, owner.node_id, topics)
        .await
        .unwrap();
    store.commit(transaction).await.unwrap();
    pool.close().await;
    start(settings).await
}
